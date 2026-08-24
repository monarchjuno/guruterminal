"""Replace in-bundle symlinks before Tauri resource staging."""

from __future__ import annotations

import argparse
import os
import shutil
import stat
import uuid
from pathlib import Path


def materialize(root: Path) -> None:
    root = root.resolve(strict=True)
    while True:
        links: list[Path] = []
        for directory, directory_names, file_names in os.walk(root, followlinks=False):
            base = Path(directory)
            for name in [*directory_names, *file_names]:
                candidate = base / name
                if candidate.is_symlink():
                    links.append(candidate)
        if not links:
            break

        for link in sorted(links):
            try:
                target = link.resolve(strict=True)
                target.relative_to(root)
            except (OSError, ValueError) as error:
                raise SystemExit(
                    f"OpenBB symlink escapes its bundle: {link}"
                ) from error
            metadata = target.stat()
            temporary = link.with_name(f".{link.name}.{uuid.uuid4().hex}.materialized")
            try:
                if stat.S_ISREG(metadata.st_mode):
                    with (
                        target.open("rb") as source,
                        temporary.open("xb") as destination,
                    ):
                        shutil.copyfileobj(source, destination)
                        destination.flush()
                        os.fsync(destination.fileno())
                    os.chmod(
                        temporary,
                        stat.S_IMODE(metadata.st_mode),
                        follow_symlinks=False,
                    )
                elif stat.S_ISDIR(metadata.st_mode):
                    shutil.copytree(target, temporary, symlinks=True)
                    os.chmod(
                        temporary,
                        stat.S_IMODE(metadata.st_mode),
                        follow_symlinks=False,
                    )
                else:
                    raise SystemExit(
                        f"OpenBB symlink target is not a file or directory: {link}"
                    )
                os.replace(temporary, link)
            finally:
                if temporary.is_dir():
                    shutil.rmtree(temporary)
                else:
                    temporary.unlink(missing_ok=True)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("bundle", type=Path)
    arguments = parser.parse_args()
    materialize(arguments.bundle)
    return 0
