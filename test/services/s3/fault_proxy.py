#!/usr/bin/env python3
"""MinIO proxy that fails CompleteMultipartUpload for one test prefix.

Used by test/sql/services/s3.test to force a multipart-completion failure while
still allowing the abort, proving no orphaned uploads remain. Runs as the
`fault-proxy` service in test/services/s3/docker-compose.yml.

Configuration (env, with standalone-friendly defaults):
  FAULT_PROXY_UPSTREAM_HOST   upstream MinIO host      (default 127.0.0.1)
  FAULT_PROXY_UPSTREAM_PORT   upstream MinIO port      (default 19100)
  FAULT_PROXY_BIND_HOST       address to listen on     (default 127.0.0.1)
  FAULT_PROXY_BIND_PORT       port to listen on        (default 19101)
  FAULT_PROXY_FAIL_PREFIX     path prefix to fail      (default /warehouse/abort-test/)
"""

import os
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit

UPSTREAM_HOST = os.environ.get("FAULT_PROXY_UPSTREAM_HOST", "127.0.0.1")
UPSTREAM_PORT = int(os.environ.get("FAULT_PROXY_UPSTREAM_PORT", "19100"))
BIND_HOST = os.environ.get("FAULT_PROXY_BIND_HOST", "127.0.0.1")
BIND_PORT = int(os.environ.get("FAULT_PROXY_BIND_PORT", "19101"))
FAIL_PREFIX = os.environ.get("FAULT_PROXY_FAIL_PREFIX", "/warehouse/abort-test/")


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        self.proxy()

    def do_HEAD(self):
        self.proxy()

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
