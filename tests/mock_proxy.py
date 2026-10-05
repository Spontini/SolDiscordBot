from http.server import BaseHTTPRequestHandler, HTTPServer


class Fixture(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header('Content-Length', '13')
        self.end_headers()
        self.wfile.write(b'proxy-fixture')
    def log_message(self, *_): pass


HTTPServer(('127.0.0.1', 40000), Fixture).serve_forever()
