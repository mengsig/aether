"""Offline invariants for the generated TypeScript module layout."""
import re
import unittest

import generate_clients as generator
from generate_clients_typescript import declarations, exported_names, files


class TypeScriptLayoutTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.schema = generator.load(generator.SCHEMA_PATH)
        cls.definitions = {
            name: generator.typescript_definition(name, value, cls.schema["$defs"])
            for name, value in sorted(cls.schema["$defs"].items())
        }
        cls.commands = generator.typescript_commands(cls.schema["$defs"])
        cls.output = files(cls.schema, cls.definitions, cls.commands, [generator.BANNER])

    def test_every_module_is_bounded(self):
        for path, content in self.output.items():
            with self.subTest(path=path):
                self.assertLessEqual(len(content.splitlines()), 600)

    def test_every_export_is_preserved_exactly_once(self):
        expected = []
        for lines in [*self.definitions.values(), self.commands]:
            expected.extend(exported_names(lines))
        actual = []
        for content in self.output.values():
            actual.extend(exported_names(content.splitlines()))
        self.assertCountEqual(actual, expected)
        self.assertEqual(len(actual), len(set(actual)))

    def test_declarations_and_documentation_are_unchanged(self):
        modules = list(self.output.values())
        for lines in [*self.definitions.values(), self.commands]:
            for declaration in declarations(lines):
                text = "\n".join(declaration).strip()
                self.assertTrue(any(text in module for module in modules), text[:120])

    def test_facade_reexports_every_module(self):
        paths = re.findall(r'export \* from "\./([^"]+)";', self.output["protocol.ts"])
        self.assertCountEqual(paths, set(self.output) - {"protocol.ts"})

    def test_output_is_deterministic(self):
        self.assertEqual(self.output, files(
            self.schema, self.definitions, self.commands, [generator.BANNER]))

    def test_an_image_reference_block_is_typed_with_its_metadata(self):
        module = self.output["protocol/types/ContentBlock.ts"]
        start = module.index("export interface ContentBlockImageRef {")
        interface = module[start:module.index("}", start)]
        for field in ('"id": AttachmentId;', '"media_type": string;',
                      '"name"?: string | null;', '"size_bytes": number;',
                      '"type": "image_ref";'):
            self.assertIn(field, interface)
        self.assertIn("AttachmentId,", module[:start])


if __name__ == "__main__":
    unittest.main()
