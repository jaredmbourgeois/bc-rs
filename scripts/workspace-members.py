"""The members of bc-rs, read from what Cargo resolves rather than from a list kept by hand.

    python3 scripts/workspace-members.py order <workspace-root>
    python3 scripts/workspace-members.py packages <workspace-root> <member-directory>

A member is a directory directly under the workspace root where Cargo builds a package, so a clone of a release —
which carries none of the working line's declarations — has the same members. Everything is derived from
`cargo metadata`:

- `order` prints the members one per line, each after every member it depends on, so the root pipeline's first red
  names the lowest break. Members that do not depend on each other keep alphabetical order.
- `packages` prints the names of the packages under one member, one per line, for its pipeline's `-p` arguments.
Three hand-kept lists (the root manifest's members, the root pipeline's run order, and each member's packages) used
to be compared in one direction only; deriving two of them from the third is what keeps them from disagreeing.

Exit codes: 0 answered; 2 the workspace could not be read, which is never an answer; 64 a malformed command line.
"""

import json
import os
import subprocess
import sys
from dataclasses import dataclass


class Unreadable(Exception):
    """The workspace could not be read, so nothing about it can be answered."""


@dataclass(frozen=True)
class Package:
    name: str
    directory: str
    # The directories of the packages this one depends on by path, resolved by Cargo.
    path_dependencies: tuple[tuple[str, str], ...]


def packages_of(metadata: dict) -> list[Package]:
    """The workspace's packages, as `cargo metadata --no-deps` reports them."""
    packages: list[Package] = []
    for package in metadata.get("packages", []):
        dependencies = tuple(
            (dependency["name"], os.path.realpath(dependency["path"]))
            for dependency in package.get("dependencies", [])
            if dependency.get("path")
        )
        packages.append(Package(
            name=package["name"],
            directory=os.path.realpath(os.path.dirname(package["manifest_path"])),
            path_dependencies=dependencies,
        ))
    if not packages:
        raise Unreadable("cargo metadata listed no packages")
    return packages


def is_within(directory: str, parent: str) -> bool:
    return os.path.commonpath([directory, parent]) == parent


def member_of(package: Package, members: list[str]) -> list[str]:
    """The member directories a package sits under: exactly one, when the workspace is sound."""
    return [member for member in members if is_within(package.directory, member)]


def order(packages: list[Package], members: list[str]) -> list[str]:
    """The members, each after the members it depends on; ties broken by name, so the order is the same every run."""
    owner: dict[str, str] = {}
    for package in packages:
        owners = member_of(package, members)
        if len(owners) != 1:
            raise Unreadable(f"the package {package.name} does not sit under exactly one member")
        owner[package.directory] = owners[0]
    depends_on: dict[str, set[str]] = {member: set() for member in members}
    for package in packages:
        for _dependency, directory in package.path_dependencies:
            if directory not in owner:
                raise Unreadable(f"{package.name} depends by path on {directory}, which is no package of bc-rs")
            if owner[directory] != owner[package.directory]:
                depends_on[owner[package.directory]].add(owner[directory])
    ordered: list[str] = []
    remaining = sorted(members)
    while remaining:
        ready = [member for member in remaining if depends_on[member] <= set(ordered)]
        if not ready:
            raise Unreadable(f"the members depend on each other in a cycle: {', '.join(remaining)}")
        ordered.append(ready[0])
        remaining.remove(ready[0])
    return ordered


def member_directories(root: str, packages: list[Package]) -> list[str]:
    """The directories directly under `root` where Cargo builds a package; an empty answer reads nothing."""
    members = sorted({package.directory for package in packages if os.path.dirname(package.directory) == root})
    if not members:
        raise Unreadable("Cargo builds no package directly under the workspace root, so there is no member to read")
    return members


def read_metadata(root: str) -> dict:
    command = ["cargo", "metadata", "--format-version", "1", "--no-deps", "--locked", "--offline",
               "--manifest-path", os.path.join(root, "Cargo.toml")]
    completed = subprocess.run(command, capture_output=True, text=True, check=False)
    if completed.returncode != 0:
        raise Unreadable(f"cargo metadata exited {completed.returncode}: {completed.stderr.strip()}")
    try:
        return json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise Unreadable(f"cargo metadata printed something that is not JSON: {error}") from error


def main(argv: list[str]) -> int:
    if len(argv) < 2 or argv[0] not in ("order", "packages") \
            or (argv[0] == "packages") != (len(argv) == 3) or len(argv) > 3:
        print(__doc__.strip().split("\n\n")[0], file=sys.stderr)
        return 64
    verb = argv[0]
    root = os.path.realpath(argv[1])
    try:
        packages = packages_of(read_metadata(root))
        members = member_directories(root, packages)
        if verb == "order":
            for member in order(packages, members):
                print(os.path.basename(member))
            return 0
        member = os.path.realpath(argv[2])
        if member not in members:
            raise Unreadable(f"{argv[2]} is not a member of bc-rs (Cargo builds no package directly there)")
        names = sorted(package.name for package in packages if member_of(package, members) == [member])
        if not names:
            raise Unreadable(f"no package sits under {argv[2]}")
        for name in names:
            print(name)
        return 0
    except Unreadable as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
