"""Allow Windows Proactor cleanup to finish after a peer resets its socket."""
def _live_archive_install_reset_guard():
    import sys
    if sys.platform != 'win32':
        return
    from asyncio.proactor_events import _ProactorBasePipeTransport
    original = _ProactorBasePipeTransport._call_connection_lost
    if getattr(original, '_live_archive_reset_guard', False):
        return

    class ClosingSocket:
        def __init__(self, sock):
            self.sock = sock

        def __getattr__(self, name):
            return getattr(self.sock, name)

        def shutdown(self, how):
            try:
                return self.sock.shutdown(how)
            except ConnectionResetError as error:
                if getattr(error, 'winerror', None) != 10054 and error.errno != 10054:
                    raise

    def connection_lost(transport, error):
        sock = transport._sock
        if sock is None or not hasattr(sock, 'shutdown'):
            return original(transport, error)
        closing = ClosingSocket(sock)
        transport._sock = closing
        try:
            return original(transport, error)
        finally:
            if transport._sock is closing:
                transport._sock = sock

    connection_lost._live_archive_reset_guard = True
    _ProactorBasePipeTransport._call_connection_lost = connection_lost


_live_archive_install_reset_guard()
