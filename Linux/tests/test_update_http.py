"""Reproduce GitHub's HTTP 415 response to binary Accept headers on JSON APIs."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import sys
import tempfile
from threading import Thread
import unittest
from unittest.mock import patch
from urllib.error import HTTPError

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, os.environ.get('ANYWHERE_TEST_UI', str(ROOT / 'Linux')))
from anywhere_host import updates


class UpdateHTTP(unittest.TestCase):
    def test_release_discovery_uses_json_and_packages_use_binary_accept(self):
        name = 'anywhere-host_0.1.6-1_amd64.deb'
        assets = [{'name': name + suffix, 'browser_download_url':
                   updates.REPOSITORY + '/releases/download/v0.1.6/' + name + suffix}
                  for suffix in ('', '.sig')]

        class GitHubContract(BaseHTTPRequestHandler):
            def do_GET(self):
                api = self.path == '/releases/latest'
                expected = 'application/vnd.github+json' if api else 'application/octet-stream'
                if self.headers.get('Accept') != expected:
                    self.send_error(415)
                    return
                body = json.dumps({'assets': assets}).encode() if api else b'package fixture'
                self.send_response(200)
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *args):
                pass

        server = ThreadingHTTPServer(('127.0.0.1', 0), GitHubContract)
        worker = Thread(target=server.serve_forever, daemon=True)
        worker.start()
        base = 'http://127.0.0.1:' + str(server.server_port)
        try:
            with tempfile.TemporaryDirectory() as temporary:
                destination = Path(temporary) / 'download'
                with self.assertRaises(HTTPError) as error:
                    updates.download(base + '/releases/latest', destination, 4096)
                self.assertEqual(error.exception.code, 415)
                with patch.object(updates, 'API', base + '/releases/latest'), \
                        patch.object(updates, 'native_architecture', return_value='amd64'), \
                        patch.object(updates.Updater, 'current_version', return_value='0.1.5-1'):
                    updater = updates.Updater(None)
                    updater.check()
                    self.assertEqual(updater.candidate['name'], name)
                updates.download(base + '/package.deb', destination, 4096)
                self.assertEqual(destination.read_bytes(), b'package fixture')
        finally:
            server.shutdown()
            server.server_close()
            worker.join()
