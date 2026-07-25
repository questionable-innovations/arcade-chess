"""PlatformIO extra_scripts hook exposing the repo tools as project tasks.

Registered targets appear in the PlatformIO sidebar under Project Tasks ->
<environment> -> Custom, and run as `pio run -e <env> -t <name>`. Registration
is gated on PIOENV so the fuses/bringup environments (which extend the base
quadrant environment) stay uncluttered.
"""

import subprocess
from pathlib import Path

Import("env")  # noqa: F821  (SCons construction environment)

# Single project-wide clock switch. Change only this value, then rebuild and
# re-provision every quadrant. Valid values: "internal" or "external".
QUADRANT_CLOCK = "internal"

CLOCK_PROFILES = {
    "internal": {"f_cpu": "8000000L", "oscillator": "internal", "label": "8 MHz internal RC"},
    "external": {"f_cpu": "16000000L", "oscillator": "external", "label": "16 MHz crystal"},
}
if QUADRANT_CLOCK not in CLOCK_PROFILES:
    raise ValueError(f"invalid QUADRANT_CLOCK: {QUADRANT_CLOCK}")
CLOCK_PROFILE = CLOCK_PROFILES[QUADRANT_CLOCK]

# This is a pre-build script: update the board settings before the Arduino core,
# application, fuse image, or bootloader targets are configured.
env.BoardConfig().update("build.f_cpu", CLOCK_PROFILE["f_cpu"])
env.BoardConfig().update("hardware.oscillator", CLOCK_PROFILE["oscillator"])
env.Replace(BOARD_F_CPU=CLOCK_PROFILE["f_cpu"])

# Pre-build scripts run before the AVR builder renames PROGNAME from the
# "program" default to "firmware", and AddCustomTarget resolves `dependencies`
# eagerly. Apply the same rename here so the targets depend on the hex file that
# actually gets built.
if env.get("PROGNAME", "program") == "program":
    env.Replace(PROGNAME="firmware")
TARGET_HEX = env.subst("$BUILD_DIR/${PROGNAME}.hex")

REPO_ROOT = Path(env["PROJECT_DIR"]).resolve()
while REPO_ROOT.parent != REPO_ROOT and not (REPO_ROOT / "tools" / "pio-targets.py").is_file():
    REPO_ROOT = REPO_ROOT.parent
if not (REPO_ROOT / "tools" / "pio-targets.py").is_file():
    raise RuntimeError("could not locate arcade-chess repository root")
TOOLS = REPO_ROOT / "tools"
PROTOCOL_TESTS = REPO_ROOT / "protocol" / "test" / "run-host-tests.sh"
QUADRANT_COUNT = 4

# The AVR has no stack guard: everything from `_end` to RAMEND is stack, and an
# overflow silently rewrites the sensor/calibration globals instead of crashing.
# The deepest call chain (main -> sendFrame -> Print::write -> HardwareSerial)
# measures 438 bytes and the deepest ISR adds 19, so this is the real floor plus
# a little slack — not a round number to be raised when a build trips it.
AVR_RAMEND = 0x08FF
AVR_MINIMUM_FREE_RAM = 480


def _end_address(elf: str) -> int:
    toolchain = Path(env.PioPlatform().get_package_dir("toolchain-atmelavr"))
    output = subprocess.run(
        [str(toolchain / "bin" / "avr-nm"), elf],
        check=True, capture_output=True, text=True,
    ).stdout
    for line in output.splitlines():
        fields = line.split()
        if len(fields) == 3 and fields[2] == "_end":
            return int(fields[0], 16) & 0xFFFF
    raise RuntimeError(f"no _end symbol in {elf}")


def check_stack_headroom(source, target, env) -> None:  # noqa: ARG001
    # `checkprogsize` is an alias, so its target node is not the ELF.
    end = _end_address(env.subst("$BUILD_DIR/${PROGNAME}$PROGSUFFIX"))
    free = AVR_RAMEND + 1 - end
    # SCons pipes its own output separately, so an unflushed print lands out of
    # order in build logs.
    print(f"Stack headroom: {free} bytes free above _end (0x{end:04x})", flush=True)
    if free < AVR_MINIMUM_FREE_RAM:
        print(
            f"error: only {free} bytes between _end and RAMEND; "
            f"{AVR_MINIMUM_FREE_RAM} are needed for the stack. "
            "Shrink a .bss object or a stack frame rather than raising this.",
            flush=True,
        )
        env.Exit(1)


def add_protocol_tests() -> None:
    env.AddCustomTarget(
        name="protocol_tests",
        dependencies=None,
        actions=[f'sh "{PROTOCOL_TESTS}"'],
        title="Protocol host tests",
        description="Build and run the shared framing/CRC tests on the host",
    )


if env["PIOPLATFORM"] == "atmelavr":
    env.AddPostAction(
        "checkprogsize",
        env.VerboseAction(check_stack_headroom, "Checking stack headroom"),
    )

if env["PIOENV"] == "ATmega328PB":
    env.AddCustomTarget(
        name="flash_all_quadrants_simultaneous",
        dependencies=TARGET_HEX,
        actions=[f'"{TOOLS}/flash-quadrant.py" --simultaneous --hex "{TARGET_HEX}"'],
        title="Flash all quadrants simultaneously (ESP USB)",
        description="Program every attached quadrant from one shared Urprotocol stream",
    )
    for node in range(QUADRANT_COUNT):
        env.AddCustomTarget(
            name=f"flash_quadrant_{node}",
            dependencies=TARGET_HEX,
            actions=[f'"{TOOLS}/flash-quadrant.py" --node {node} --hex "{TARGET_HEX}"'],
            title=f"Flash quadrant {node} (ESP USB)",
            description="Build, then program via the ESP console fw-flash path",
        )
    env.AddCustomTarget(
        name="flash_isp",
        dependencies=TARGET_HEX,
        actions=[f'"{TOOLS}/flash-isp.sh" --hex "{TARGET_HEX}"'],
        title="Flash firmware (ISP, fast)",
        description="Application and Urboot only; keeps fuses and the EEPROM identity",
    )
    for node in range(QUADRANT_COUNT):
        env.AddCustomTarget(
            name=f"provision_quadrant_{node}",
            dependencies=None,
            actions=[f'"{TOOLS}/provision-quadrant.sh" --id {node} --yes'],
            title=f"Provision quadrant {node} (ISP)",
            description=f"{CLOCK_PROFILE['label']}: fuses, Urboot, application, and EEPROM",
        )
    add_protocol_tests()
elif env["PIOENV"] == "esp32dev":
    add_protocol_tests()
