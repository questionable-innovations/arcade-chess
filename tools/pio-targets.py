"""PlatformIO extra_scripts hook exposing the repo tools as project tasks.

Registered targets appear in the PlatformIO sidebar under Project Tasks ->
<environment> -> Custom, and run as `pio run -e <env> -t <name>`. Registration
is gated on PIOENV so the fuses/bringup environments (which extend the base
quadrant environment) stay uncluttered.
"""

from pathlib import Path

Import("env")  # noqa: F821  (SCons construction environment)

REPO_ROOT = Path(env["PROJECT_DIR"]).resolve().parent
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
        name="flash_all_quadrants",
        dependencies="$BUILD_DIR/${PROGNAME}.hex",
        actions=[
            f'"{TOOLS}/flash-quadrant.py" --all --hex "$BUILD_DIR/${{PROGNAME}}.hex"'
        ],
        title="Flash all quadrants (ESP USB)",
        description="Build, then program every quadrant in sequence",
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
            description="Fuses, Urboot, application, and identity EEPROM via USBasp",
        )
    add_protocol_tests()
elif env["PIOENV"] == "esp32dev":
    add_protocol_tests()
