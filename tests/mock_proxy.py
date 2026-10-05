from http.server import BaseHTTPRequestHandler, HTTPServer
import socket
import threading


def udp_fixture():
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as server:
        server.bind(('172.30.90.1', 5353))
        while True:
            data, address = server.recvfrom(512)
            server.sendto(data, address)


class Fixture(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header('Content-Length', '13')
        self.end_headers()
        self.wfile.write(b'proxy-fixture')
    def log_message(self, *_): pass


threading.Thread(target=udp_fixture, daemon=True).start()
HTTPServer(('127.0.0.1', 40000), Fixture).serve_forever()
