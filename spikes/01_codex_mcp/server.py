# /// script
# requires-python = ">=3.12,<3.13"
# dependencies = ["mcp>=2,<3"]
# ///
"""Spike 1 stand-in for `ferb mcp`: one stdio MCP tool that returns a nonce.

The nonce comes from SPIKE_NONCE, so the harness can prove the model's answer
came from a real tool call and not from a guess. Every call is appended to
SPIKE_CALL_LOG so the harness can also see the call from the server side.
"""

import json
import os

from mcp.server.mcpserver import MCPServer

app = MCPServer("ferb-spike")


@app.tool()
def get_ticket(name: str) -> str:
    """Return the ticket code for the given name."""
    log = os.environ.get("SPIKE_CALL_LOG")
    if log:
        with open(log, "a") as f:
            f.write(json.dumps({"tool": "get_ticket", "name": name}) + "\n")
    return f"ticket for {name}: {os.environ['SPIKE_NONCE']}"


if __name__ == "__main__":
    app.run()
