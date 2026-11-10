"""ADR index tests run on throwaway ADR folders."""

import contextlib
import importlib.util
import io
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "adr_index", Path(__file__).with_name("adr_index.py")
)
adr_index = importlib.util.module_from_spec(spec)
spec.loader.exec_module(adr_index)

README = "# Architecture decision records\n\n## Index\n\n<!-- index -->\n\nold rows\n"


def adr(title, date, status="accepted"):
    return f"# {title}\n\n- **Date:** {date}\n- **Status:** {status}\n\n## Context\n"


class AdrIndexTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.dir = Path(temp.name)
        (self.dir / "README.md").write_text(README)

    def write(self, name, text):
        (self.dir / name).write_text(text)

    def run_main(self, *argv):
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            code = adr_index.main(list(argv), adr_dir=self.dir)
        return code, err.getvalue()

    def test_index_lists_numbered_and_slug_adrs_by_date(self):
        self.write("0002-old.md", adr("0002. An old decision", "2026-10-14 (also 2026-10-15)"))
        self.write("new-rule.md", adr("A new decision", "2026-10-30"))
        self.write("0001-first.md", adr("0001. The first decision", "2026-10-13"))

        self.assertEqual(self.run_main(), (0, ""))

        self.assertEqual(
            (self.dir / "README.md").read_text(),
            "# Architecture decision records\n\n## Index\n\n<!-- index -->\n\n"
            "| Decision | Date | Status |\n|---|---|---|\n"
            "| [0001. The first decision](0001-first.md) | 2026-10-13 | accepted |\n"
            "| [0002. An old decision](0002-old.md) | 2026-10-14 | accepted |\n"
            "| [A new decision](new-rule.md) | 2026-10-30 | accepted |\n",
        )

    def test_check_fails_on_a_stale_index_and_leaves_it(self):
        self.write("new-rule.md", adr("A new decision", "2026-10-30"))

        code, err = self.run_main("--check")

        self.assertEqual(code, 1)
        self.assertIn("the index is stale", err)
        self.assertEqual((self.dir / "README.md").read_text(), README)

    def test_check_passes_on_a_written_index(self):
        self.write("new-rule.md", adr("A new decision", "2026-10-30"))
        self.run_main()

        self.assertEqual(self.run_main("--check"), (0, ""))

    def test_a_new_adr_with_a_number_is_refused(self):
        self.write("0054-next.md", adr("0054. Next", "2026-10-31"))

        code, err = self.run_main()

        self.assertEqual(code, 1)
        self.assertIn("0054-next.md: a new ADR has no number", err)

    def test_an_adr_without_its_date_line_is_refused(self):
        self.write("broken.md", "# Broken\n\n- **Status:** accepted\n")

        with self.assertRaisesRegex(ValueError, "broken.md"):
            self.run_main()


if __name__ == "__main__":
    unittest.main()
