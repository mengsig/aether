"""Tests of the quality scoring sheet: offline, no model."""
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "quality"))
import score_sheet


class SheetTest(unittest.TestCase):
    def write(self, tmp, name, data):
        path = Path(tmp, name)
        path.write_text(json.dumps(data))
        return str(path)

    def test_a_sheet_lists_every_dimension_for_every_transcript(self):
        with tempfile.TemporaryDirectory() as tmp:
            one = self.write(tmp, "a.json", {"id": "a", "messages": [{"role": "teacher", "phase": "plan", "text": "hi"}]})
            two = self.write(tmp, "b.json", {"messages": [{"role": "learner", "text": "x"}]})
            sheet = score_sheet.render_sheet(json.loads(score_sheet.RUBRIC.read_text()),
                                             [score_sheet.load_transcript(one), score_sheet.load_transcript(two)])
        self.assertIn("transcript a ", sheet)
        self.assertIn("transcript b ", sheet)
        self.assertEqual(sheet.count("Probing efficiency"), 2)
        self.assertEqual(sheet.count("Question quality"), 2)

    def test_a_malformed_transcript_is_an_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            bad = self.write(tmp, "bad.json", {"messages": [{"role": "teacher"}]})
            with self.assertRaises(score_sheet.TranscriptError):
                score_sheet.load_transcript(bad)
            empty = self.write(tmp, "empty.json", {"messages": []})
            with self.assertRaises(score_sheet.TranscriptError):
                score_sheet.load_transcript(empty)

    def test_main_returns_2_for_a_missing_file(self):
        self.assertEqual(score_sheet.main(["/nonexistent/t.json"]), 2)


if __name__ == "__main__":
    unittest.main()
