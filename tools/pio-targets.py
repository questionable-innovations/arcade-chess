"""PlatformIO extra_scripts hook exposing the repo tools as project tasks.

Registered targets appear in the PlatformIO sidebar under Project Tasks ->
<environment> -> Custom, and run as `pio run -e <env> -t <name>`. Registration
is gated on PIOENV so the fuses/bringup environments (which extend the base
quadrant environment) stay uncluttered.
"""

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

REPO_ROOT = Path(env["PROJECT_DIR"]).resolve()
while REPO_ROOT.parent != REPO_ROOT and not (REPO_ROOT / "tools" / "pio-targets.py").is_file():
    REPO_ROOT = REPO_ROOT.parent
if not (REPO_ROOT / "tools" / "pio-targets.py").is_file():
    raise RuntimeError("could not locate arcade-chess repository root")
TOOLS = REPO_ROOT / "tools"
PROTOCOL_TESTS = REPO_ROOT / "protocol" / "test" / "run-host-tests.sh"
QUADRANT_COUNT = 4


def add_protocol_tests() -> None:
    env.AddCustomTarget(
        name="protocol_tests",
        dependencies=None,
        actions=[f'sh "{PROTOCOL_TESTS}"'],
        title="Protocol host tests",
        description="Build and run the shared framing/CRC tests on the host",
    )


if env["PIOENV"] == "ATmega328PB":
    env.AddCustomTarget(
        name="flash_all_quadrants_simultaneous",
        dependencies="$BUILD_DIR/${PROGNAME}.hex",
        actions=[
            f'"{TOOLS}/flash-quadrant.py" --simultaneous --hex "$BUILD_DIR/${{PROGNAME}}.hex"'
        ],
        title="Flash all quadrants simultaneously (ESP USB)",
        description="Program every attached quadrant from one shared Urprotocol stream",
    )
    for node in range(QUADRANT_COUNT):
        env.AddCustomTarget(
            name=f"flash_quadrant_{node}",
            dependencies="$BUILD_DIR/${PROGNAME}.hex",
            actions=[
                f'"{TOOLS}/flash-quadrant.py" --node {node} '
                '--hex "$BUILD_DIR/${PROGNAME}.hex"'
            ],
            title=f"Flash quadrant {node} (ESP USB)",
            description="Build, then program via the ESP console fw-flash path",
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
