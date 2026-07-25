#!/usr/bin/env python3
"""Provision the Azurite container used by the azblob:// test tier.

Creates the test container and uploads the shared external-read fixtures with a
different client than opendal (plain Azure Blob REST + Shared Key signing), so
`common/external_read.test` really exercises foreign-written objects.

Only the Python standard library is used: the init sidecar runs on a plain
`python:3-alpine` image, exactly like the s3 tier's fault proxy. The Azure CLI
image would work too but costs a >1 GB pull on every CI run.

Env (all have emulator defaults; the account/key are Azurite's public
well-known credentials):

  AZURITE_ENDPOINT      blob endpoint incl. the account path segment
  AZURITE_ACCOUNT_NAME  storage account name
  AZURITE_ACCOUNT_KEY   base64 shared key
  AZURITE_CONTAINERS    space-separated containers to create
  AZURITE_FIXTURES_DIR  local dir uploaded to <first container>/external/
"""

import base64
import hashlib
import hmac
import os
import sys
import urllib.error
import urllib.request
from email.utils import formatdate

# Azurite's public, well-known development credentials (not a secret).
DEFAULT_ACCOUNT = "devstoreaccount1"
DEFAULT_KEY = "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw=="
API_VERSION = "2021-08-06"

ENDPOINT = os.environ.get("AZURITE_ENDPOINT", "http://azurite:10000/devstoreaccount1")
ACCOUNT = os.environ.get("AZURITE_ACCOUNT_NAME", DEFAULT_ACCOUNT)
KEY = os.environ.get("AZURITE_ACCOUNT_KEY", DEFAULT_KEY)
CONTAINERS = os.environ.get("AZURITE_CONTAINERS", "warehouse vault").split()
FIXTURES_DIR = os.environ.get("AZURITE_FIXTURES_DIR", "/fixtures")


def _canonicalized_resource(path, query):
    """`/<account><path>` plus one sorted `\\n<key>:<value>` line per query param."""
    resource = "/" + ACCOUNT + path
    for key in sorted(query):
        resource += "\n" + key.lower() + ":" + query[key]
    return resource


def _authorization(verb, path, query, headers, content_length):
    """Azure Storage Shared Key (full, not Lite) authorization header."""
    canonicalized_headers = "".join(
        f"{name}:{value}\n"
        for name, value in sorted(
            (k.lower(), v) for k, v in headers.items() if k.lower().startswith("x-ms-")
        )
    )
    string_to_sign = "\n".join(
        [
            verb,
            "",  # Content-Encoding
            "",  # Content-Language
            str(content_length) if content_length else "",
            "",  # Content-MD5
            headers.get("Content-Type", ""),
            "",  # Date (x-ms-date is used instead)
            "",  # If-Modified-Since
            "",  # If-Match
            "",  # If-None-Match
            "",  # If-Unmodified-Since
            "",  # Range
        ]
    )
    string_to_sign += (
        "\n" + canonicalized_headers + _canonicalized_resource(path, query)
    )
    signature = base64.b64encode(
        hmac.new(
            base64.b64decode(KEY), string_to_sign.encode("utf-8"), hashlib.sha256
        ).digest()
    ).decode()
    return f"SharedKey {ACCOUNT}:{signature}"


def request(
    verb, path, query=None, body=None, extra_headers=None, ok_status=(200, 201, 202)
):
    """Issue one signed Blob REST call. Returns the HTTP status code."""
    query = query or {}
    body = body or b""
    headers = {
        "x-ms-date": formatdate(usegmt=True),
        "x-ms-version": API_VERSION,
    }
    headers.update(extra_headers or {})
    if body:
        # urllib would otherwise inject `application/x-www-form-urlencoded`,
        # which then differs from the Content-Type used in the string-to-sign.
        headers.setdefault("Content-Type", "application/octet-stream")
    headers["Authorization"] = _authorization(verb, path, query, headers, len(body))
    headers["Content-Length"] = str(len(body))

    url = ENDPOINT.rsplit("/" + ACCOUNT, 1)[0] + path
    if query:
        url += "?" + "&".join(f"{k}={v}" for k, v in sorted(query.items()))

    req = urllib.request.Request(url, data=body or None, headers=headers, method=verb)
    try:
        with urllib.request.urlopen(req) as response:
            status = response.status
    except urllib.error.HTTPError as err:
        status = err.code
        if status not in ok_status:
            sys.exit(
                f"{verb} {url} failed: {status} {err.read().decode(errors='replace')}"
            )
    if status not in ok_status:
        sys.exit(f"{verb} {url} failed: {status}")
    return status


def create_container(name):
    # 409 = ContainerAlreadyExists — provisioning must be idempotent.
    request(
        "PUT",
        f"/{ACCOUNT}/{name}",
        query={"restype": "container"},
        ok_status=(201, 409),
    )


def upload_fixtures(container):
    if not os.path.isdir(FIXTURES_DIR):
        return 0
    uploaded = 0
    for entry in sorted(os.listdir(FIXTURES_DIR)):
        local = os.path.join(FIXTURES_DIR, entry)
        if not os.path.isfile(local):
            continue
        with open(local, "rb") as handle:
            body = handle.read()
        request(
            "PUT",
            f"/{ACCOUNT}/{container}/external/{entry}",
            body=body,
            extra_headers={"x-ms-blob-type": "BlockBlob"},
            ok_status=(201,),
        )
        uploaded += 1
    return uploaded


def main():
    for container in CONTAINERS:
        create_container(container)
        print(f"container ready: {container}")
    uploaded = upload_fixtures(CONTAINERS[0])
    print(f"uploaded {uploaded} external fixture(s) to {CONTAINERS[0]}/external/")
    print("FINISHED SETTING UP AZURITE")


if __name__ == "__main__":
    main()
