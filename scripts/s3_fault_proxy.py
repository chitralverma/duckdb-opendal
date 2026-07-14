#!/usr/bin/env python3
"""MinIO proxy that fails CompleteMultipartUpload for one test prefix."""

from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit

UPSTREAM_HOST = "127.0.0.1"
UPSTREAM_PORT = 19100
FAIL_PREFIX = "/warehouse/abort-test/"


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
        headers = {
            key: value
            for key, value in self.headers.items()
            if key.lower() != "connection"
        }
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
    ThreadingHTTPServer(("127.0.0.1", 19101), Handler).serve_forever()
