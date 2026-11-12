import pathlib
import subprocess


root = pathlib.Path(__file__).resolve().parents[1]
subprocess.run(
    ["cargo", "test", "-p", "cordis-abi", "--test", "unsafe_audit"],
    cwd=root,
    check=True,
)
