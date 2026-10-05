"""Run as UID 10001 inside the actual protected media container in Linux CI."""
import os
import socket
import urllib.request


def blocked(host, port):
    try:
        connection = socket.create_connection((host, port), timeout=1)
    except OSError:
        return
    connection.close()
    raise AssertionError(f'direct connection unexpectedly allowed: {host}:{port}')


# Public IPv4, IPv4-mapped IPv6 and other ports on the bridge gateway are blocked.
blocked('1.1.1.1', 443)
blocked('::ffff:1.1.1.1', 443)
blocked('172.30.90.1', 40000)
blocked('172.30.90.3', 8080)
with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as dns:
    dns.settimeout(1)
    dns.sendto(b'\x00' * 12, ('1.1.1.1', 53))
    try:
        dns.recv(512)
        raise AssertionError('direct UDP DNS unexpectedly allowed')
    except socket.timeout:
        pass

# A host-side fixture is reachable only through the permitted relay port.
opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
with opener.open('http://172.30.90.1:40001/fixture', timeout=5) as response:
    assert response.read() == b'proxy-fixture'

# Docker exec is UID 10001; verify the real running worker dropped all capabilities.
for entry in os.listdir('/proc'):
    if entry.isdigit():
        try:
            command = open(f'/proc/{entry}/cmdline', 'rb').read()
            if b'/opt/media/worker.py' in command or b'/opt/bgutil/build/main.js' in command:
                fields = dict(line.split(':', 1) for line in open(f'/proc/{entry}/status') if ':' in line)
                assert int(fields['CapEff'].strip(), 16) == 0
                assert int(fields['CapBnd'].strip(), 16) == 0
        except (FileNotFoundError, PermissionError):
            pass
print('Direct IPv4/IPv6/DNS blocked; proxy relay reachable; application capabilities dropped.')
