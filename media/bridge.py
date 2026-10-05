"""Private bridge to host WARP's loopback proxy; never an Internet-facing proxy."""
import concurrent.futures
import os
import selectors
import socket
import time


def relay(client):
    with client:
        client.settimeout(10)
        try:
            with socket.create_connection(('127.0.0.1', 40000), timeout=5) as upstream:
                client.setblocking(False)
                upstream.setblocking(False)
                with selectors.DefaultSelector() as selector:
                    selector.register(client, selectors.EVENT_READ, upstream)
                    selector.register(upstream, selectors.EVENT_READ, client)
                    while events := selector.select(timeout=120):
                        for key, _ in events:
                            block = key.fileobj.recv(65536)
                            if not block:
                                return
                            # Bound memory; backpressure by waiting for the destination.
                            remaining = memoryview(block)
                            until = time.monotonic() + 30
                            while remaining:
                                try:
                                    remaining = remaining[key.data.send(remaining):]
                                except BlockingIOError:
                                    if time.monotonic() >= until:
                                        return
                                    time.sleep(0.005)
        except OSError:
            return


def main():
    listener = socket.socket()
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    # This interface exists only once Docker creates the private bridge.
    for attempt in range(60):
        try:
            listener.bind((os.environ['SOL_BRIDGE_IP'], 40001))
            break
        except OSError:
            if attempt == 59:
                raise
            time.sleep(1)
    listener.listen(16)
    allowed = os.environ['SOL_WORKER_IP']
    # No unbounded executor queue: a slot is acquired before accepting a client.
    with concurrent.futures.ThreadPoolExecutor(max_workers=16) as pool:
        import threading
        slots = threading.BoundedSemaphore(16)
        while True:
            slots.acquire()
            client, address = listener.accept()
            if address[0] != allowed:
                client.close()
                slots.release()
                continue
            future = pool.submit(relay, client)
            future.add_done_callback(lambda _: slots.release())


if __name__ == '__main__':
    main()
