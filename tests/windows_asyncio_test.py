from pathlib import Path
from types import SimpleNamespace
import sys
import unittest
from unittest.mock import patch
from asyncio.proactor_events import _ProactorBasePipeTransport as Transport

SOURCE = (Path(__file__).resolve().parents[1] / 'assets/moderation/proxy/windows_asyncio.py').read_text(encoding='utf-8')


class CleanupTests(unittest.TestCase):
    def setUp(self):
        self.original = Transport._call_connection_lost

    def tearDown(self):
        Transport._call_connection_lost = self.original

    def install(self):
        with patch.object(sys, 'platform', 'win32'):
            exec(SOURCE, {})

    def transport(self, error=None, protocol_error=None):
        events = []
        def shutdown(how):
            events.append('shutdown')
            if error:
                raise error
        def lost(error):
            events.append('protocol')
            if protocol_error:
                raise protocol_error
        sock = SimpleNamespace(fileno=lambda: 1, shutdown=shutdown, close=lambda: events.append('close'))
        transport = SimpleNamespace(_sock=sock, _called_connection_lost=False,
            _protocol=SimpleNamespace(connection_lost=lost),
            # CPython 3.12 calls _detach(), while 3.14 passes the transport.
            _server=SimpleNamespace(_detach=lambda *args: events.append('detach')))
        return transport, sock, events

    def test_reset_still_closes_and_detaches_exactly_once(self):
        self.install()
        transport, _, events = self.transport(ConnectionResetError(10054, 'peer reset'))
        Transport._call_connection_lost(transport, None)
        Transport._call_connection_lost(transport, None)
        self.assertEqual(events, ['protocol', 'shutdown', 'close', 'detach'])
        self.assertIsNone(transport._sock)
        self.assertIsNone(transport._server)
        self.assertTrue(transport._called_connection_lost)

    def test_other_errors_are_not_silenced(self):
        self.install()
        for error in [ConnectionResetError(10053, 'other'), OSError(5, 'denied')]:
            with self.subTest(error=error):
                transport, sock, events = self.transport(error)
                with self.assertRaises(type(error)):
                    Transport._call_connection_lost(transport, None)
                self.assertIs(transport._sock, sock)
                self.assertEqual(events, ['protocol', 'shutdown'])

    def test_protocol_failure_propagates_after_socket_cleanup(self):
        self.install()
        transport, _, events = self.transport(ConnectionResetError(10054, 'reset'), RuntimeError('protocol error'))
        with self.assertRaisesRegex(RuntimeError, 'protocol error'):
            Transport._call_connection_lost(transport, None)
        self.assertEqual(events, ['protocol', 'shutdown', 'close', 'detach'])

    def test_install_is_idempotent_and_skips_other_platforms(self):
        with patch.object(sys, 'platform', 'darwin'):
            exec(SOURCE, {})
        self.assertIs(Transport._call_connection_lost, self.original)
        self.install()
        installed = Transport._call_connection_lost
        self.install()
        self.assertIs(Transport._call_connection_lost, installed)


if __name__ == '__main__':
    unittest.main()
