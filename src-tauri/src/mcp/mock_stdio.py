"""Local test fixture only; never launched by production MCP code."""
import json
import os
import sys

# Fixed synthetic text exercises the Rust credential filter without sending
# any value read from the environment. Keep these in sync with mcp/tests.rs.
FIRST_FIXTURE_VALUE = "mock-secret-value"
SECOND_FIXTURE_VALUE = 'mock-"quoted\\env'

for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    if "id" not in request:
        continue
    if method == "initialize":
        result = {
            "protocolVersion": "2025-03-26",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "local-mock", "version": "1"},
        }
    elif method == "tools/list":
        result = {"tools": [{"name": "echo", "description": "mock", "inputSchema": {"type": "object"}}]}
    elif method == "tools/call":
        result = {"content": [{"type": "text", "text": json.dumps({
            "pid": os.getpid(),
            "firstCredentialMatches": os.environ.get("GLOSSMOD_API_KEY") == FIRST_FIXTURE_VALUE,
            "secondCredentialMatches": os.environ.get("SECOND_API_KEY") == SECOND_FIXTURE_VALUE,
            "firstFixtureValue": FIRST_FIXTURE_VALUE,
            "secondFixtureValue": SECOND_FIXTURE_VALUE,
            "ambientSecretPresent": "AMM_TEST_AMBIENT_SECRET" in os.environ,
            "arguments": request.get("params", {}).get("arguments"),
        })}]}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
