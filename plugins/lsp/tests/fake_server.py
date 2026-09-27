"""A fake language server for the lsp plugin's tests in this directory.

It reports "found error" at the first "error" of each line, answers hover
with where the cursor is, and puts every definition at line 0, character 3
of the same file. Positions are in UTF-8 bytes, as it tells the client.
"""
import json
import sys


def read():
    """The next message, or None when the client closed stdin."""
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        name, _, value = line.partition(b":")
        if name.lower() == b"content-length":
            length = int(value)
    return json.loads(sys.stdin.buffer.read(length))


def send(message):
    body = json.dumps(message).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()


def reply(request, result):
    send({"jsonrpc": "2.0", "id": request["id"], "result": result})


def diagnostics(text):
    found = []
    for line, content in enumerate(text.encode().split(b"\n")):
        at = content.find(b"error")
        if at >= 0:
            found.append({
                "range": {
                    "start": {"line": line, "character": at},
                    "end": {"line": line, "character": at + 5},
                },
                "severity": 1,
                "message": "found error",
            })
    return found


def publish(uri, text):
    send({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": {"uri": uri, "diagnostics": diagnostics(text)},
    })


def offset(text, position):
    lines = text.split(b"\n")
    start = sum(len(line) + 1 for line in lines[: position["line"]])
    return start + position["character"]


def main():
    documents = {}
    while (message := read()) is not None:
        params = message.get("params", {})
        uri = params.get("textDocument", {}).get("uri", "")
        method = message.get("method")
        if method == "initialize":
            reply(message, {"capabilities": {
                "positionEncoding": "utf-8",
                "textDocumentSync": 2,
                "hoverProvider": True,
                "definitionProvider": True,
            }})
        elif method == "textDocument/didOpen":
            documents[uri] = params["textDocument"]["text"].encode()
            publish(uri, documents[uri].decode())
        elif method == "textDocument/didChange":
            text = documents.get(uri, b"")
            for change in params["contentChanges"]:
                new = change["text"].encode()
                if "range" in change:
                    start = offset(text, change["range"]["start"])
                    end = offset(text, change["range"]["end"])
                    text = text[:start] + new + text[end:]
                else:
                    text = new
            documents[uri] = text
            publish(uri, text.decode())
        elif method == "textDocument/hover":
            at = params["position"]
            value = f"hover at {at['line']}:{at['character']}"
            reply(message, {"contents": {"kind": "plaintext", "value": value}})
        elif method == "textDocument/definition":
            reply(message, {"uri": uri, "range": {
                "start": {"line": 0, "character": 3},
                "end": {"line": 0, "character": 7},
            }})
        elif method == "shutdown":
            reply(message, None)
        elif method == "exit":
            return


main()
