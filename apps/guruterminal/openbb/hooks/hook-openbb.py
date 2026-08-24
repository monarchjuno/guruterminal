"""Freeze every installed OpenBB extension and its entry-point metadata.

This hook copies only metadata needed by the running process. The build's
separate THIRD_PARTY_LICENSES archive owns complete legal metadata without
changing importlib's frozen runtime discovery surface.
"""

from importlib.metadata import packages_distributions

from PyInstaller.utils.hooks import collect_all, copy_metadata

_package_owners = packages_distributions()
_openbb_distributions = sorted(
    {
        distribution
        for owners in _package_owners.values()
        for distribution in owners
        if distribution.lower().startswith("openbb")
    }
)
_openbb_packages = sorted(
    {
        package
        for package, owners in _package_owners.items()
        if any(owner in _openbb_distributions for owner in owners)
    }
)
_runtime_metadata = ("fastmcp", "fastmcp-slim", "mcp")
_runtime_data_packages = {"random_user_agent": "random-user-agent"}


def _include_data(item):
    destination = "/" + item[1].replace("\\", "/")
    return "openbb_mcp_server/skills" not in destination and "/tests" not in destination


datas = []
binaries = []
hiddenimports = []

for package in _openbb_packages:
    package_datas, package_binaries, package_imports = collect_all(
        package,
        include_py_files=True,
    )
    datas.extend(item for item in package_datas if _include_data(item))
    binaries.extend(package_binaries)
    hiddenimports.extend(
        name
        for name in package_imports
        if "tests" not in name.split(".")
        and not name.rsplit(".", 1)[-1].startswith("test_")
    )

for package, distribution in _runtime_data_packages.items():
    package_datas, package_binaries, package_imports = collect_all(
        package,
        include_py_files=True,
    )
    datas.extend(package_datas)
    binaries.extend(package_binaries)
    hiddenimports.extend(package_imports)
    datas.extend(copy_metadata(distribution))

for distribution in _openbb_distributions:
    datas.extend(copy_metadata(distribution))
for distribution in _runtime_metadata:
    datas.extend(copy_metadata(distribution))

datas = list(dict.fromkeys(datas))
binaries = list(dict.fromkeys(binaries))
hiddenimports = list(dict.fromkeys(hiddenimports))
