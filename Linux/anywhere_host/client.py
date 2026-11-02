"""Local management API: TLS 1.3, exact leaf pinning, v2 admin proofs."""
import base64
import hashlib
import hmac
import http.client
import ipaddress
import json
import secrets
import ssl
from urllib.parse import urlencode, urlsplit


class HostClient:
    def __init__(self, directory):
        self.key = (directory / 'admin-key-v2').read_text().strip().encode()
        self.certificate = ssl.PEM_cert_to_DER_cert((directory / 'host-cert.pem').read_text())
        self.fingerprint = hashlib.sha256(self.certificate).hexdigest()
        config = json.loads((directory / 'config.json').read_text())
        host, port = config.get('listen', '127.0.0.1:9443').rsplit(':', 1)
        address = ipaddress.ip_address(host.strip('[]'))
        # Management credentials never leave this machine, even if config is edited.
        if address.is_unspecified:
            address = ipaddress.ip_address('::1' if address.version == 6 else '127.0.0.1')
        if not address.is_loopback:
            raise ValueError('GTK 管理界面要求 listen 为回环或通配地址')
        self.host, self.port = str(address), int(port)

    def _request(self, path, body=None, headers=None):
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        context.check_hostname = False
        context.verify_mode = ssl.CERT_NONE  # Replaced by exact certificate pin below.
        context.minimum_version = ssl.TLSVersion.TLSv1_3
        connection = http.client.HTTPSConnection(self.host, self.port, context=context, timeout=25)
        try:
            connection.connect()
            if not hmac.compare_digest(connection.sock.getpeercert(binary_form=True), self.certificate):
                raise ssl.SSLError('Host 证书与本地证书不匹配')
            # No redirect following, proxies, cookies or credentials before pinning.
            connection.request('POST' if body is not None else 'GET', path, body, headers or {})
            response = connection.getresponse()
            raw = response.read(2 * 1024 * 1024 + 1)
            if len(raw) > 2 * 1024 * 1024:
                raise ValueError('Host 响应过大')
            value = json.loads(raw)
            if not 200 <= response.status < 300:
                raise RuntimeError(str(value.get('error', f'HTTP {response.status}')))
            return value
        finally:
            connection.close()

    def request(self, path, body=None):
        raw = json.dumps(body, separators=(',', ':')).encode() if body is not None else None
        challenge = self._request('/v1/security/challenge')
        timestamp, nonce = str(challenge['serverTime']), secrets.token_hex(16)
        digest = hashlib.sha256(raw or b'').hexdigest()
        canonical = '\n'.join(['Anywhere-v2', self.fingerprint, 'Anywhere-Admin',
                               'POST' if raw is not None else 'GET', path, digest,
                               timestamp, nonce, challenge['challenge']]).encode()
        return self._request(path, raw, {
            'Content-Type': 'application/json', 'Authorization': 'Anywhere-Admin',
            'X-Anywhere-Content-SHA256': digest, 'X-Anywhere-Time': timestamp,
            'X-Anywhere-Nonce': nonce, 'X-Anywhere-Challenge': challenge['challenge'],
            'X-Anywhere-Signature': base64.b64encode(hmac.digest(self.key, canonical, 'sha256')).decode(),
        })

    def pairing_link(self, endpoint):
        url = urlsplit(endpoint)
        if (url.scheme != 'https' or not url.hostname or url.username or url.password
                or url.path not in ('', '/') or url.query or url.fragment
                or any(c.isspace() for c in endpoint)):
            raise ValueError('请输入 https://主机:端口 格式的远程地址')
        _ = url.port  # Validate the port before issuing a ticket.
        ticket = self.request('/v1/security/ticket', {})
        link = 'anywhere-host://connect?' + urlencode({
            'v': '2', 'endpoint': endpoint, 'ticket': ticket['ticket'],
            'certificateSHA256': self.fingerprint,
        })
        return link, ticket['expiresAt']
