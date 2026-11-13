#!/usr/bin/env python3
"""MinIO proxy that fails CompleteMultipartUpload for one test prefix and
counts requests per method.

Used by test/sql/services/s3.test to force a multipart-completion failure while
still allowing the abort, proving no orphaned uploads remain, and to assert how
many requests a statement issued. Runs as the `fault-proxy` service in
test/services/s3/docker-compose.yml.

  GET /__counts            -> CSV "method,count" per HTTP method seen so far
  GET /__counts/reset      -> zero the counters, then respond like /__counts

Configuration (env, with standalone-friendly defaults):
  FAULT_PROXY_UPSTREAM_HOST   upstream MinIO host      (default 127.0.0.1)
  FAULT_PROXY_UPSTREAM_PORT   upstream MinIO port      (default 19100)
  FAULT_PROXY_BIND_HOST       address to listen on     (default 127.0.0.1)
  FAULT_PROXY_BIND_PORT       port to listen on        (default 19101)
  FAULT_PROXY_FAIL_PREFIX     path prefix to fail      (default /warehouse/abort-test/)
"""

import os
import threading
from collections import Counter
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit

UPSTREAM_HOST = os.environ.get("FAULT_PROXY_UPSTREAM_HOST", "127.0.0.1")
UPSTREAM_PORT = int(os.environ.get("FAULT_PROXY_UPSTREAM_PORT", "19100"))
BIND_HOST = os.environ.get("FAULT_PROXY_BIND_HOST", "127.0.0.1")
BIND_PORT = int(os.environ.get("FAULT_PROXY_BIND_PORT", "19101"))
FAIL_PREFIX = os.environ.get("FAULT_PROXY_FAIL_PREFIX", "/warehouse/abort-test/")

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
        self.send_response(200)
        self.send_header("Content-Type", "text/csv")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if write_body:
            self.wfile.write(body)

    def do_PUT(self):
        self.proxy()

    def do_POST(self):
        query = parse_qs(urlsplit(self.path).query, keep_blank_values=True)
        if self.path.startswith(FAIL_PREFIX) and "uploadId" in query:
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
        with COUNTS_LOCK:
            COUNTS[self.command] += 1
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length) if length else None
        headers = {key: value for key, value in self.headers.items() if key.lower() != "connection"}
        connection = HTTPConnection(UPSTREAM_HOST, UPSTREAM_PORT, timeout=30)
        try:
            connection.request(self.command, self.path, body=body, headers=headers)
            response = connection.getresponse()
            payload = response.read()
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


if __name__ == "__main__":
    ThreadingHTTPServer((BIND_HOST, BIND_PORT), Handler).serve_forever()
