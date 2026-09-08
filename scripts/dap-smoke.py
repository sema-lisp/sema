#!/usr/bin/env python3
"""Drive a real `sema dap` process without an editor or third-party packages.

Build first: cargo build -p sema-lang
Example:
  python3 scripts/dap-smoke.py --program examples/async-worker-pool.sema \
    --break-text '(channel/send results' --evaluate job --evaluate id \
    --expect-output 'sum of results =  650'
"""

import argparse
from collections import deque
import json
from pathlib import Path
import queue
import subprocess
import threading
import time


class DapClient:
    def __init__(self, binary, cwd, timeout):
        self.process = subprocess.Popen(
            [str(binary), "dap"], cwd=cwd,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        self.timeout = timeout
        self.seq = 0
        self.last_action = "starting adapter"
        self.messages = queue.Queue()
        self.events = deque()
        self.output = []
        self.stderr = []
        self.readers = [threading.Thread(target=self._read, daemon=True),
                        threading.Thread(target=self._read_stderr, daemon=True)]
        for reader in self.readers:
            reader.start()

    def _read_stderr(self):
        for line in self.process.stderr:
            self.stderr.append(line.decode("utf-8", errors="replace"))

    def _read(self):
        try:
            while True:
                headers = {}
                while True:
                    line = self.process.stdout.readline()
                    if not line:
                        raise EOFError("adapter closed stdout")
                    if line == b"\r\n":
                        break
                    name, value = line.decode("ascii").split(":", 1)
                    headers[name.lower()] = value.strip()
                size = int(headers["content-length"])
                if not 0 <= size <= 16 * 1024 * 1024:
                    raise ValueError("invalid DAP frame size")
                body = self.process.stdout.read(size)
                if len(body) != size:
                    raise EOFError("truncated DAP body")
                self.messages.put(json.loads(body))
        except Exception as error:
            self.messages.put(error)

    def _next(self, deadline):
        try:
            message = self.messages.get(timeout=max(0, deadline - time.monotonic()))
        except queue.Empty as error:
            raise TimeoutError("timed out waiting for the adapter") from error
        if isinstance(message, Exception):
            raise message
        if message.get("event") == "output":
            self.output.append(message["body"])
        return message

    def request(self, command, arguments=None):
        self.last_action = f"request {command}"
        self.seq += 1
        body = json.dumps({"seq": self.seq, "type": "request",
                           "command": command, "arguments": arguments or {}}).encode()
        self.process.stdin.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
        self.process.stdin.flush()
        deadline = time.monotonic() + self.timeout
        while True:
            message = self._next(deadline)
            if message.get("type") == "response":
                assert message["request_seq"] == self.seq, message
                assert message["command"] == command, message
                assert message["success"], message
                return message.get("body", {})
            self.events.append(message)

    def event(self, name):
        self.last_action = f"event {name}"
        deadline = time.monotonic() + self.timeout
        while True:
            message = self.events.popleft() if self.events else self._next(deadline)
            if message.get("event") == name:
                return message.get("body", {})
            if message.get("event") == "terminated":
                raise AssertionError(f"program terminated before {name}: {self.output}")

    def frames(self):
        return self.request("stackTrace", {"threadId": 1})["stackFrames"]

    def close(self):
        if self.process.poll() is None:
            self.process.terminate()
        try:
            self.process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
        for reader in self.readers:
            reader.join(timeout=1)
        for stream in (self.process.stdin, self.process.stdout, self.process.stderr):
            stream.close()


def source_line(path, text):
    matches = [number for number, line in enumerate(path.read_text().splitlines(), 1)
               if text in line]
    if len(matches) != 1:
        raise ValueError(f"expected one source line matching {text!r}, got {matches}")
    return matches[0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/sema"))
    parser.add_argument("--program", type=Path, required=True)
    parser.add_argument("--break-text", required=True)
    parser.add_argument("--evaluate", action="append", default=[])
    parser.add_argument("--expect-output")
    parser.add_argument("--finish-text", help="stop before this line instead of running to termination")
    parser.add_argument("--finish-evaluate", action="append", default=[])
    parser.add_argument("--timeout", type=float, default=15)
    parser.add_argument("--verbose-output", action="store_true")
    parser.add_argument("--zero-based", action="store_true", help="request zero-based DAP lines and columns")
    parser.add_argument("--uri-paths", action="store_true", help="request file URI source paths")
    parser.add_argument("--expect-next-line", type=int)
    parser.add_argument("--expect-evaluate", action="append", default=[], metavar="EXPRESSION=RESULT")
    parser.add_argument("--expect-finish", action="append", default=[], metavar="EXPRESSION=RESULT")
    args = parser.parse_args()
    program = args.program.resolve()
    client_path = program.as_uri() if args.uri_paths else str(program)
    line_offset = int(args.zero_based)
    line = source_line(program, args.break_text)
    finish_line = source_line(program, args.finish_text) if args.finish_text else None
    client = DapClient(args.binary.resolve(), program.parent, args.timeout)
    report = {"program": str(program), "breakpoint_line": line}
    report["client"] = {"zero_based": args.zero_based, "uri_paths": args.uri_paths}
    try:
        client.request("initialize", {"adapterID": "sema-smoke", "linesStartAt1": not args.zero_based,
                                      "columnsStartAt1": not args.zero_based,
                                      "pathFormat": "uri" if args.uri_paths else "path"})
        client.event("initialized")
        client.request("setBreakpoints", {"source": {"path": client_path},
                                          "breakpoints": [{"line": line - line_offset}]})
        client.request("launch", {"program": client_path})
        client.request("configurationDone")
        assert client.event("stopped")["reason"] == "breakpoint"
        frames = client.frames()
        assert frames[0]["line"] == line - line_offset, frames
        assert frames[0]["source"]["path"] == client_path, frames
        report["frames"] = frames
        scopes = client.request("scopes", {"frameId": frames[0]["id"]})["scopes"]
        report["locals"] = []
        for scope in scopes:
            if scope["name"].lower() == "locals":
                report["locals"] = client.request("variables", {
                    "variablesReference": scope["variablesReference"],
                })["variables"]
        report["evaluated"] = {expr: client.request("evaluate", {
            "expression": expr, "frameId": frames[0]["id"], "context": "watch",
        })["result"] for expr in args.evaluate}
        for check in args.expect_evaluate:
            expression, expected = check.rsplit("=", 1)
            assert report["evaluated"][expression] == expected, report["evaluated"]
        client.request("setBreakpoints", {"source": {"path": client_path},
            "breakpoints": [{"line": finish_line - line_offset}] if finish_line else []})
        client.request("next", {"threadId": 1})
        assert client.event("stopped")["reason"] == "step"
        report["after_next"] = client.frames()
        if args.expect_next_line is not None:
            assert report["after_next"][0]["line"] == args.expect_next_line - line_offset, report["after_next"]
        client.request("continue", {"threadId": 1})
        if finish_line:
            assert client.event("stopped")["reason"] == "breakpoint"
            frames = client.frames()
            assert frames[0]["line"] == finish_line - line_offset, frames
            report["finish_evaluated"] = {expr: client.request("evaluate", {
                "expression": expr, "frameId": frames[0]["id"], "context": "watch",
            })["result"] for expr in args.finish_evaluate}
            for check in args.expect_finish:
                expression, expected = check.rsplit("=", 1)
                assert report["finish_evaluated"][expression] == expected, report["finish_evaluated"]
            report["completion"] = f"stopped before line {finish_line}"
        else:
            client.event("terminated")
            report["completion"] = "terminated event received"
        report["output"] = client.output if args.verbose_output else client.output[-5:]
        report["output_event_count"] = len(client.output)
        if args.expect_output:
            stdout = "".join(item["output"] for item in client.output
                             if item["category"] == "stdout")
            assert args.expect_output in stdout, stdout
        client.request("disconnect")
        assert client.process.wait(timeout=3) == 0
        report["passed"] = True
    except Exception as error:
        report.update(passed=False, error=str(error), last_action=client.last_action,
                      output=client.output[-10:],
                      adapter_stderr="".join(client.stderr))
    finally:
        client.close()
    print(json.dumps(report, indent=2))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
