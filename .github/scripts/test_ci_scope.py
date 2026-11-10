"""The required check must not hide a failed job or skip cross-language contracts."""

import importlib.util
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location("ci_scope", Path(__file__).with_name("ci_scope.py"))
ci = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ci)


class Selection(unittest.TestCase):
    def test_documentation_only_needs_repository_checks(self):
        self.assertEqual(ci.scope(["docs/SPEC.md", "README.md", "LICENSE"]),
                         {"rust": False, "javascript": False})

    def test_ui_and_sidecar_changes_do_not_rebuild_rust(self):
        self.assertEqual(ci.scope(["app/src/App.tsx", "sidecar/src/sessions.ts", "relay/bun.lock"]),
                         {"rust": False, "javascript": True})

    def test_shared_contracts_and_native_app_changes_run_both(self):
        for path in ["protocol-ts/src/generated/Workspace.ts", "sidecar/src/version.ts",
                     "sidecar/src/package-crypto.ts", "sidecar/src/agents/context-files.ts",
                     "app/src-tauri/src/main.rs"]:
            with self.subTest(path=path):
                self.assertEqual(ci.scope([path]), {"rust": True, "javascript": True})

    def test_unknown_paths_build_inputs_and_ci_changes_run_both(self):
        for path in ["new-component/code.py", "Cargo.lock", "crates/chauffeurd/src/lib.rs",
                     ".github/workflows/core.yml", ".github/scripts/ci_scope.py"]:
            with self.subTest(path=path):
                self.assertEqual(ci.scope([path]), {"rust": True, "javascript": True})

    def test_renamed_files_include_both_old_and_new_locations(self):
        self.assertEqual(ci.scope(["crates/old.rs", "docs/old.rs"]),
                         {"rust": True, "javascript": True})


class RequiredCheck(unittest.TestCase):
    def needs(self, rust="success", javascript="success", selected="true", changes="success"):
        return {"changes": {"result": changes, "outputs": {"rust": selected, "javascript": selected}},
                "rust": {"result": rust}, "javascript": {"result": javascript}}

    def test_all_selected_checks_must_succeed(self):
        ci.validate(self.needs())
        for result in ["failure", "cancelled", "skipped"]:
            for job in ["rust", "javascript"]:
                with self.subTest(result=result, job=job), self.assertRaises(ValueError):
                    ci.validate(self.needs(**{job: result}))

    def test_only_deliberately_unselected_jobs_may_be_skipped(self):
        ci.validate(self.needs(rust="skipped", javascript="skipped", selected="false"))
        with self.assertRaises(ValueError):
            ci.validate(self.needs(rust="failure", javascript="skipped", selected="false"))

    def test_failed_or_cancelled_change_detection_never_passes(self):
        for result in ["failure", "cancelled", "skipped"]:
            with self.subTest(result=result), self.assertRaises(ValueError):
                ci.validate(self.needs(changes=result))


if __name__ == "__main__":
    unittest.main()
