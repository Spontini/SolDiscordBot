"""Bounded subprocess I/O and credential-free operational logging."""
import json
import logging
import logging.handlers
import os
import subprocess
import threading


def configure_logging():
    os.makedirs('/var/log/sol', exist_ok=True)
    handler = logging.handlers.RotatingFileHandler(
        '/var/log/sol/media.log', maxBytes=1024 * 1024, backupCount=3)
    logging.basicConfig(level=logging.INFO, handlers=[handler], format='%(message)s')


def event(name, **fields):
    # Callers supply fixed event names, counts, status codes and exception TYPES.
    # Never pass exception text, upstream output, URLs, headers or token values.
    logging.info(json.dumps({'event': name, **fields}, separators=(',', ':')))


def run_bounded(command, *, data=None, timeout=60, limit=2 * 1024 * 1024):
    """Kill/reap on timeout or oversized output; discard potentially secret stderr."""
    with subprocess.Popen(command, stdin=subprocess.PIPE if data else subprocess.DEVNULL,
                          stdout=subprocess.PIPE, stderr=subprocess.DEVNULL) as child:
        output = bytearray()
        overflow = threading.Event()

        def drain():
            while block := child.stdout.read(8192):
                if len(output) + len(block) > limit:
                    overflow.set()
                    child.kill()
                    return
                output.extend(block)

        reader = threading.Thread(target=drain, daemon=True)
        reader.start()
        try:
            if data:
                child.stdin.write(data)
                child.stdin.close()
            status = child.wait(timeout=timeout)
        except BaseException:
            child.kill()
            child.wait()
            raise
        finally:
            reader.join(timeout=3)
        if overflow.is_set() or reader.is_alive():
            raise ValueError('output_limit')
        if status:
            raise RuntimeError('subprocess_failed')
        return bytes(output)
