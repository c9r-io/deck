import importlib.util
import pathlib
import sys
import unittest


PATH = pathlib.Path(__file__).with_name("edr_runtime.py")
SPEC = importlib.util.spec_from_file_location("edr_runtime", PATH)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class EdrRuntimeTests(unittest.TestCase):
    def test_process_and_smoke_tmux_are_parsed_without_metadata_payload(self):
        executable = MODULE.DEBUG_BUNDLES / "deck-smoke.app/Contents/MacOS/tmux"
        process = MODULE.parse_ps_line(
            f'240 1 09-00:14:24 {executable} '
            '-f /tmp/conf -L deck-smoke-safe start-server ; set-option -g '
            '@deck-server-metadata {"source":"smoke"}'
        )
        self.assertIsNotNone(process)
        self.assertEqual(
            MODULE.tmux_candidate(process),
            (
                str(executable),
                "deck-smoke-safe",
            ),
        )

    def test_production_and_arbitrary_sockets_are_never_candidates(self):
        for socket in ["deck", "default", "deck-smoke", "deck-dev-other"]:
            process = MODULE.parse_ps_line(
                f"10 1 00:01 /Applications/deck.app/Contents/MacOS/tmux -L {socket} start-server"
            )
            self.assertIsNone(MODULE.tmux_candidate(process), socket)

    def test_cleanup_requires_matching_closed_metadata(self):
        self.assertTrue(MODULE.cleanup_allowed("deck-smoke-one", "smoke"))
        self.assertTrue(MODULE.cleanup_allowed("deck-dev", "development"))
        self.assertFalse(MODULE.cleanup_allowed("deck", "installed"))
        self.assertFalse(MODULE.cleanup_allowed("deck-smoke-one", "development"))
        self.assertFalse(MODULE.cleanup_allowed("deck-dev", "smoke"))

    def test_app_ownership_is_exact_to_the_smoke_socket(self):
        executable = MODULE.DEBUG_BUNDLES / "deck-smoke.app/Contents/MacOS/deck"
        process = MODULE.parse_ps_line(
            f"99 1 00:01 {executable} --smoke-data-dir /tmp/root "
            "--smoke-tmux-socket deck-smoke-one"
        )
        app = MODULE.managed_app(process)
        self.assertIsNotNone(app)
        self.assertEqual(app.socket, "deck-smoke-one")

    def test_production_app_is_not_managed(self):
        process = MODULE.parse_ps_line(
            "99 1 00:01 /Applications/deck.app/Contents/MacOS/deck-app"
        )
        self.assertIsNone(MODULE.managed_app(process))

    def test_lookalike_bundle_outside_the_repository_is_not_executed(self):
        process = MODULE.parse_ps_line(
            "10 1 00:01 /tmp/deck-smoke.app/Contents/MacOS/tmux "
            "-L deck-smoke-lookalike start-server"
        )
        self.assertIsNone(MODULE.tmux_candidate(process))


if __name__ == "__main__":
    unittest.main()
