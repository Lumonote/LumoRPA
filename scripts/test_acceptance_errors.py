import contextlib
import io
import os
import unittest
from unittest.mock import patch

from acceptance_errors import error_annotation, run_acceptance


class AnnotationTests(unittest.TestCase):
    def test_failure_messages_cannot_inject_another_workflow_command(self):
        message = "original failure\n::notice::forged\r100%"
        result = error_annotation("native, Linux:test", message)
        self.assertEqual(result, "::error title=native%2C Linux%3Atest::original failure%0A::notice::forged%0D100%25")
        self.assertEqual(len(result.splitlines()), 1)

    def test_ci_annotation_preserves_original_exception(self):
        failure = ValueError("missing native dependency")
        def fail():
            raise failure
        output = io.StringIO()
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "true"}), contextlib.redirect_stdout(output):
            with self.assertRaises(ValueError) as caught:
                run_acceptance(fail, "Native recorder")
        self.assertIs(caught.exception, failure)
        self.assertEqual(output.getvalue(), "::error title=Native recorder::ValueError: missing native dependency\n")

    def test_local_failures_do_not_emit_ci_commands_and_success_returns_result(self):
        def fail():
            raise RuntimeError("local")
        output = io.StringIO()
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "false"}), contextlib.redirect_stdout(output):
            with self.assertRaises(RuntimeError):
                run_acceptance(fail, "fixture")
            self.assertEqual(run_acceptance(lambda: 42, "fixture"), 42)
        self.assertEqual(output.getvalue(), "")


if __name__ == "__main__":
    unittest.main()
