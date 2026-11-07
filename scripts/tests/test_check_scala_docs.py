"""Tests of scripts/check-scala-docs.py."""
import importlib.util
import os
import unittest

spec = importlib.util.spec_from_file_location(
    "check_scala_docs", os.path.join(os.path.dirname(__file__), "..", "check-scala-docs.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

DOCUMENTED = '''/** A thing. */
object Thing:
  /** Does it. */
  def run(): Unit =
    val local = 1
    def helper() = local
    helper()

  private def hidden(): Unit = ()

  /** A record. */
  @deprecated
  final case class Pair(a: Int)
'''


class CheckTest(unittest.TestCase):
    def test_a_documented_file_is_clean_and_locals_are_ignored(self):
        self.assertEqual(list(module.undocumented(DOCUMENTED)), [])

    def test_missing_docs_are_found_at_top_level_and_in_members(self):
        text = "object Thing:\n  def run(): Unit = ()\n\nclass Other\n"
        found = [declaration for _, declaration in module.undocumented(text)]
        self.assertEqual(found, ["object Thing:", "def run(): Unit = ()", "class Other"])

    def test_override_and_private_members_are_skipped(self):
        text = "/** x */\nclass A:\n  override def toString = \"a\"\n  protected val v = 1\n"
        self.assertEqual(list(module.undocumented(text)), [])

    def test_the_benchmark_modules_are_documented(self):
        root = os.path.join(os.path.dirname(__file__), "..", "..", "jvm")
        for module_name in ("adb-bench", "adb-benchmark"):
            for directory, _, files in os.walk(os.path.join(root, module_name, "src", "main")):
                for name in files:
                    if name.endswith(".scala"):
                        path = os.path.join(directory, name)
                        with open(path, encoding="utf-8") as handle:
                            text = handle.read()
                        self.assertEqual(list(module.undocumented(text)), [], path)


if __name__ == "__main__":
    unittest.main()
