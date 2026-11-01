"""FERB's own HTTP listeners on this Mac's loopback address (the shell's egress proxy, the runtimes' tool-list proxy,
the memory dashboard)."""

import http.server
import socketserver


class LocalHTTPServer(http.server.ThreadingHTTPServer):
    """A threading HTTP server that binds without asking the resolver anything.

    http.server's own bind looks the bound address up for a full host name (socket.getfqdn, a reverse DNS query). On a
    Mac whose resolver is slow or unreachable that takes seconds, and the daemon builds its proxy on its event loop:
    on GitHub's macOS runner the first start stalled past 10 s in it (2026-11-01). The name it finds is only CGI's
    SERVER_NAME, which none of these servers use, so it is the bound address instead."""

    daemon_threads = True

    def server_bind(self) -> None:
        socketserver.TCPServer.server_bind(self)
        host, port = self.server_address[:2]
        self.server_name = str(host)
        self.server_port = int(port)
