"""Local test fixture only; never launched by production MCP code."""
import json
import os
import sys

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
            "secretPresent": os.environ.get("GLOSSMOD_API_KEY") == "mock-secret-value",
            "echoedCredential": os.environ.get("GLOSSMOD_API_KEY"),
            "secondCredential": os.environ.get("SECOND_API_KEY"),
            "ambientSecretPresent": "AMM_TEST_AMBIENT_SECRET" in os.environ,
            "arguments": request.get("params", {}).get("arguments"),
        })}]}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
