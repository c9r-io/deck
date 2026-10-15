"""The source-to-catalog boundary, including failures before either output changes."""

import importlib.util
import json
import pathlib
import tempfile
import unittest


SPEC = importlib.util.spec_from_file_location("extension_catalog", pathlib.Path(__file__).with_name("extension_catalog.py"))
catalog = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(catalog)


class CatalogTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="deck-extension-catalog-")
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        (self.root / "app/extensions").mkdir(parents=True)
        (self.root / "app/ui/js").mkdir(parents=True)
        self.add("engine", "translation", 90, selected=True)

    def add(self, identity, kind="agent", order=10, selected=False):
        entry = dict(id=identity, kind=kind, rust=f"crate::test::{identity.upper().replace('-', '_')}",
                     ui=f"extension-{identity}.js", order=order, selected=selected)
        self.write(entry)
        (self.root / "app/ui/js" / entry["ui"]).write_text(f"export const CONTRIBUTION = {{ id: '{identity}' }};\n")
        return entry

    def write(self, entry):
        (self.root / "app/extensions" / (entry["id"] + ".json")).write_text(json.dumps(entry))

    def snapshot(self):
        return {path: (self.root / path).read_bytes() for path in (catalog.RUST, catalog.JS)}

    def test_new_manifest_registers_both_sides_without_editing_central_tables(self):
        catalog.compose(self.root)
        entry = self.add("extra")
        catalog.compose(self.root)
        output = self.snapshot()
        self.assertIn(entry["rust"].encode(), output[catalog.RUST])
        self.assertIn(entry["ui"].encode(), output[catalog.JS])
        self.assertIn(b'"extra"', output[catalog.RUST])
        self.assertEqual(catalog.compose(self.root, check=True), [])
        # Removing the declaration and its module removes both registrations.
        (self.root / "app/extensions/extra.json").unlink()
        (self.root / "app/ui/js/extension-extra.js").unlink()
        catalog.compose(self.root)
        self.assertNotIn(b"crate::test::EXTRA", self.snapshot()[catalog.RUST])
        self.assertNotIn(b"extension-extra.js", self.snapshot()[catalog.JS])

    def test_explicit_order_is_independent_of_filename_and_idempotent(self):
        self.add("z-first", order=1)
        self.add("a-second", order=2)
        self.assertEqual([e["id"] for e in catalog.load(self.root)], ["z-first", "a-second", "engine"])
        catalog.compose(self.root)
        before = self.snapshot()
        times = [(self.root / p).stat().st_mtime_ns for p in before]
        self.assertEqual(catalog.compose(self.root), [])
        self.assertEqual(times, [(self.root / p).stat().st_mtime_ns for p in before])

    def test_check_refuses_stale_outputs_without_repairing_them(self):
        catalog.compose(self.root)
        before = self.snapshot()
        self.add("new")
        with self.assertRaisesRegex(catalog.CatalogError, "stale generated"):
            catalog.compose(self.root, check=True)
        self.assertEqual(before, self.snapshot())

    def test_invalid_declaration_never_partially_updates_catalogs(self):
        entry = self.add("new")
        catalog.compose(self.root)
        before = self.snapshot()
        cases = [
            ("kind", "command"), ("order", True), ("order", -1), ("order", 90),
            ("selected", True), ("selected", "yes"),
            ("rust", "crate::x(); unsafe {}"), ("rust", "crate::test::ENGINE"),
            ("ui", "../escape.js"), ("ui", "https://host/code.js"),
            ("ui", "extension-engine.js"), ("ui", "extension-missing.js"),
        ]
        for key, value in cases:
            with self.subTest(key=key, value=value):
                self.write({**entry, key: value})
                with self.assertRaises(catalog.CatalogError):
                    catalog.compose(self.root)
                self.assertEqual(before, self.snapshot())
        self.write(entry)
        self.write({**entry, "command": "anything"})
        with self.assertRaisesRegex(catalog.CatalogError, "expected exactly"):
            catalog.compose(self.root)

    def test_filename_identity_and_duplicate_json_keys_are_refused(self):
        path = self.root / "app/extensions/engine.json"
        content = path.read_text()
        path.write_text(content.replace('"id": "engine"', '"id": "other"'))
        with self.assertRaisesRegex(catalog.CatalogError, "filename must match"):
            catalog.load(self.root)
        path.write_text(content.replace('"id": "engine"', '"id": "engine", "id": "engine"'))
        with self.assertRaisesRegex(catalog.CatalogError, "duplicate JSON key"):
            catalog.load(self.root)

    def test_selection_must_be_exactly_one_and_never_a_runtime_override(self):
        path = self.root / "app/extensions/engine.json"
        entry = json.loads(path.read_text())
        self.write({**entry, "selected": False})
        with self.assertRaisesRegex(catalog.CatalogError, "exactly one"):
            catalog.load(self.root)
        self.write(entry)
        self.add("other", "translation", 100, selected=True)
        with self.assertRaisesRegex(catalog.CatalogError, "exactly one"):
            catalog.load(self.root)

    def test_missing_export_or_manifest_is_refused(self):
        path = self.root / "app/ui/js/extension-engine.js"
        path.write_text("export const WRONG = {};\n")
        with self.assertRaisesRegex(catalog.CatalogError, "export CONTRIBUTION"):
            catalog.load(self.root)
        path.write_text("export const CONTRIBUTION = {};\n")
        (path.parent / "extension-stray.js").write_text("export const CONTRIBUTION = {};\n")
        with self.assertRaisesRegex(catalog.CatalogError, "unregistered"):
            catalog.load(self.root)

    def test_symlink_input_and_output_are_refused(self):
        path = self.root / "app/ui/js/extension-engine.js"
        other = path.with_name("other.js")
        path.rename(other)
        path.symlink_to(other)
        with self.assertRaisesRegex(catalog.CatalogError, "regular UI module"):
            catalog.load(self.root)
        path.unlink()
        other.rename(path)
        output = self.root / catalog.JS
        output.symlink_to(path)
        with self.assertRaisesRegex(catalog.CatalogError, "must not be a symlink"):
            catalog.compose(self.root)


if __name__ == "__main__":
    unittest.main()
