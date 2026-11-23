#!/usr/bin/env python3
"""HTTP proxy in front of a service emulator that counts requests.

Each emulator tier runs it as the `proxy` service in its docker-compose.yml, so
tests can assert how many requests, and how many GET body bytes, a statement
issued.

  GET /__counts            -> CSV "method,count": requests per HTTP method seen
                              so far, plus GET_BYTES, the object data served by
                              GET (JSON and XML API responses are not counted)
  GET /__counts/reset      -> zero the counters, then respond like /__counts

Configuration (env):
  PROXY_UPSTREAM_HOST   upstream host            (default 127.0.0.1)
  PROXY_UPSTREAM_PORT   upstream port            (required)
  PROXY_BIND_HOST       address to listen on     (default 127.0.0.1)
  PROXY_BIND_PORT       port to listen on        (required)
  PROXY_FAIL_PREFIX     optional path prefix whose multipart completion (a POST
                        with uploadId) fails with a 500, so a test can prove an
                        aborted upload leaves nothing behind
"""

import os
import threading
from collections import Counter
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit

UPSTREAM_HOST = os.environ.get("PROXY_UPSTREAM_HOST", "127.0.0.1")
UPSTREAM_PORT = int(os.environ["PROXY_UPSTREAM_PORT"])
BIND_HOST = os.environ.get("PROXY_BIND_HOST", "127.0.0.1")
BIND_PORT = int(os.environ["PROXY_BIND_PORT"])
FAIL_PREFIX = os.environ.get("PROXY_FAIL_PREFIX")

COUNTS = Counter()
COUNTS_LOCK = threading.Lock()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        if self.path.startswith("/__counts"):
            self.counts(write_body=True)
            return
        self.proxy()

    def do_HEAD(self):
        if self.path.startswith("/__counts"):
            self.counts(write_body=False)
            return
        self.proxy()

    def counts(self, write_body):
        # The reset body is fixed so the HEAD a client issues before its GET
        # reports the same length the GET then serves.
        if self.path.endswith("/reset"):
            if self.command == "GET":
                with COUNTS_LOCK:
                    COUNTS.clear()
            body = b"none,0\n"
        else:
            with COUNTS_LOCK:
                body = "".join(f"{m},{c}\n" for m, c in sorted(COUNTS.items())).encode()
            if not body:
                body = b"none,0\n"
        # Clients read this like any object, with a Range header, and reject a
        # 200 to a ranged read.
        start, end = byte_range(self.headers.get("Range"), len(body))
        if start is None:
            self.send_response(200)
        else:
            self.send_response(206)
            self.send_header("Content-Range", f"bytes {start}-{end}/{len(body)}")
            body = body[start : end + 1]
        self.send_header("Content-Type", "text/csv")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if write_body:
            self.wfile.write(body)

    def do_PUT(self):
        self.proxy()

    def do_POST(self):
        query = parse_qs(urlsplit(self.path).query, keep_blank_values=True)
        if FAIL_PREFIX and self.path.startswith(FAIL_PREFIX) and "uploadId" in query:
            length = int(self.headers.get("Content-Length", "0"))
            if length:
                self.rfile.read(length)
            body = b"<Error><Code>InternalError</Code><Message>forced completion failure</Message></Error>"
            self.send_response(500)
            self.send_header("Content-Type", "application/xml")
            self.send_header("Connection", "close")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            self.close_connection = True
            return
        self.proxy()

    def do_DELETE(self):
        self.proxy()

    def proxy(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length) if length else None
        headers = {key: value for key, value in self.headers.items() if key.lower() != "connection"}
        connection = HTTPConnection(UPSTREAM_HOST, UPSTREAM_PORT, timeout=30)
        try:
            connection.request(self.command, self.path, body=body, headers=headers)
            response = connection.getresponse()
            payload = response.read()
            with COUNTS_LOCK:
                COUNTS[self.command] += 1
                kind = (response.getheader("Content-Type") or "").split(";")[0].strip()
                if self.command == "GET" and not kind.endswith(("json", "xml")):
                    COUNTS["GET_BYTES"] += len(payload)
            self.send_response(response.status, response.reason)
            for key, value in response.getheaders():
                if key.lower() not in {
                    "connection",
                    "transfer-encoding",
                    "content-length",
                }:
                    self.send_header(key, value)
            # A HEAD response carries no body; keep the upstream Content-Length
            # so clients see the object size, not 0.
            if self.command == "HEAD":
                upstream_length = response.getheader("Content-Length")
                if upstream_length is not None:
                    self.send_header("Content-Length", upstream_length)
            else:
                self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            if self.command != "HEAD":
                self.wfile.write(payload)
        finally:
            connection.close()

    def log_message(self, _format, *_args):
        pass


def byte_range(header, size):
    """The (start, end) of a single `bytes=start-[end]` range, or (None, None)."""
    if not header or not header.startswith("bytes=") or "," in header:
        return None, None
    start, _, end = header[len("bytes=") :].partition("-")
    if not start.isdigit() or (end and not end.isdigit()) or int(start) >= size:
        return None, None
    return int(start), min(int(end), size - 1) if end else size - 1


if __name__ == "__main__":
    ThreadingHTTPServer((BIND_HOST, BIND_PORT), Handler).serve_forever()
