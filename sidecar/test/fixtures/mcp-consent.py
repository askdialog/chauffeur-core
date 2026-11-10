"""Synthetic MCP consent only: stdio messages, no application or network access."""

import json
import sys

pending = {}
for line in sys.stdin:
    message = json.loads(line)
    if "method" not in message:
        tool_id = pending.pop(str(message.get("id")), None)
        if tool_id is not None:
            print(json.dumps({"jsonrpc": "2.0", "id": tool_id, "result": {
                "content": [{"type": "text", "text": json.dumps(message.get("result"))}]
            }}), flush=True)
        continue
    if "id" not in message:
        continue
    method = message["method"]
    if method == "initialize":
        result = {"protocolVersion": message["params"]["protocolVersion"], "capabilities": {"tools": {}}, "serverInfo": {"name": "consent_qa", "version": "1"}}
    elif method == "tools/list":
        result = {"tools": [{"name": "confirm", "description": "Simulated consent only; no app access.", "inputSchema": {"type": "object", "properties": {}}}]}
    elif method == "tools/call":
        request_id = "consent-" + str(message["id"])
        pending[request_id] = message["id"]
        print(json.dumps({"jsonrpc": "2.0", "id": request_id, "method": "elicitation/create", "params": {
            "mode": "form", "message": "Simulated access only", "requestedSchema": {
                "type": "object", "properties": {"approved": {"type": "boolean"}}, "required": ["approved"], "additionalProperties": False
            }
        }}), flush=True)
        continue
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": result}), flush=True)
