from http.server import BaseHTTPRequestHandler, HTTPServer
import socket
import threading

CONNECT_SEEN = threading.Event()


def udp_fixture():
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as server:
        server.bind(('172.30.90.1', 5353))
        while True:
            data, address = server.recvfrom(512)
            server.sendto(data, address)


class Fixture(BaseHTTPRequestHandler):
    def do_CONNECT(self):
        if self.path == 'example.org:443':
            CONNECT_SEEN.set()
        # Reject deliberately: this fixture tests the proxy transport, not TLS.
        self.send_response(502)
        self.send_header('Content-Length', '0')
        self.end_headers()

    def do_GET(self):
        body = (b'yes' if CONNECT_SEEN.is_set() else b'no') if self.path == '/connect_seen' else b'proxy-fixture'
        self.send_response(200)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *_): pass


threading.Thread(target=udp_fixture, daemon=True).start()
HTTPServer(('127.0.0.1', 40000), Fixture).serve_forever()
