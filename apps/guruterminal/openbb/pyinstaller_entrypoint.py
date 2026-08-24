import os

# A frozen module graph can initialize FastMCP settings before Guru's bootstrap
# creates its private scratch environment. Disable its banner and PyPI version
# check before importing the server so startup never performs undeclared I/O.
os.environ["FASTMCP_CHECK_FOR_UPDATES"] = "off"
os.environ["FASTMCP_SHOW_SERVER_BANNER"] = "false"

from guruterminal_openbb.server import main


if __name__ == "__main__":
    raise SystemExit(main())
