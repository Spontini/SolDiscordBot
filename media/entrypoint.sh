#!/bin/sh
set -eu
# Install an inet (IPv4 AND IPv6) deny-by-default policy before any application.
# This is the worker's private namespace, never the host namespace.
python3 - <<'PY'
import ipaddress, os, subprocess, urllib.parse
p = urllib.parse.urlsplit(os.environ['SOL_MEDIA_PROXY'])
address = ipaddress.ip_address(p.hostname)
assert p.scheme == 'http' and address.version == 4 and address.is_private
assert not p.username and not p.password and p.port == 40001
assert not p.path and not p.query and not p.fragment
worker = ipaddress.ip_address(os.environ['SOL_WORKER_IP'])
bot = ipaddress.ip_address(os.environ['SOL_BOT_IP'])
assert worker.version == bot.version == 4 and worker.is_private and bot.is_private
policy = f'''table inet sol_media {{
 chain output {{ type filter hook output priority 0; policy drop;
  oifname "lo" accept
  ct state established,related accept
  ip daddr {address} tcp dport 40001 accept
 }}
 chain input {{ type filter hook input priority 0; policy drop;
  iifname "lo" accept
  ct state established,related accept
  ip saddr {bot} tcp dport 8080 accept
 }}
}}'''
subprocess.run(['nft', '-f', '-'], input=policy.encode(), check=True)
PY
# Drop SETUID, SETGID and NET_ADMIN from the bounding set and all capabilities.
# Child processes cannot remove the firewall even after compromising the provider.
exec setpriv --reuid=10001 --regid=10001 --clear-groups \
  --bounding-set=-all --inh-caps=-all --ambient-caps=-all --no-new-privs \
  python3 /opt/media/worker.py
