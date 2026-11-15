"""Tests for the feasibility harness, not acceptance of its candidate renderer."""
import importlib.util
from pathlib import Path
import sys
import unittest

PATH = Path(__file__).resolve().parents[1] / 'scripts/experiments/unicode_graph_probe.py'
SPEC = importlib.util.spec_from_file_location('unicode_graph_probe', PATH)
probe = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = probe
SPEC.loader.exec_module(probe)


class UnicodeGraphProbeTests(unittest.TestCase):
    def rows(self, *lines):
        return [probe.Row([(c, 36) for c in graph], oid, oid)
                for graph, oid in lines]

    def test_linear_matches_independent_parent_oracle(self):
        rows = self.rows(('* ', 'a'), ('* ', 'b'), ('* ', 'c'))
        rendered = probe.transform(rows)
        self.assertEqual(probe.oracle(rendered, {'a': ['b'], 'b': ['c'], 'c': []}), [])
        self.assertEqual(list(rendered[2].values()), ['a', 'b', 'c'])

    def test_oracle_rejects_false_connection(self):
        rendered = probe.transform(self.rows(('* ', 'a'), ('* ', 'b')))
        self.assertTrue(probe.oracle(rendered, {'a': [], 'b': []}))

    def test_oracle_rejects_missing_connection(self):
        rendered = probe.transform(self.rows(('* ', 'a'), ('  *', 'b')))
        self.assertTrue(probe.oracle(rendered, {'a': ['b'], 'b': []}))

    def test_oracle_rejects_truncated_ancestry_without_tail(self):
        rendered = probe.transform(self.rows(('* ', 'a')))
        self.assertTrue(probe.oracle(rendered, {'a': ['unloaded']}))

    def test_unsupported_syntax_is_explicit(self):
        for glyph in ('_', '-', '.', '?'):
            with self.assertRaises(probe.Unsupported):
                probe.transform(self.rows(('*' + glyph, 'a')))

    def test_merge_preserves_both_parents(self):
        rows = self.rows(('* ', 'a'), ('|\\', ''), ('| *', 'b'),
                         ('* |', 'c'), ('|/', ''), ('* ', 'd'))
        self.assertEqual(probe.oracle(probe.transform(rows),
                                     {'a': ['b', 'c'], 'b': ['d'], 'c': ['d'], 'd': []}), [])

    def test_colors_and_uncoloured_nodes_are_not_invented(self):
        rows = probe.parse('* \0a\0subject\0\n\x1b[36m|\x1b[m \0\n')
        self.assertEqual(rows[0].cells[0], ('*', None))
        self.assertEqual(rows[1].cells[0], ('|', 36))

    def test_cached_viewport_slicing_is_repeatable(self):
        rows = self.rows(('* ', 'a'), ('|\\', ''), ('| *', 'b'),
                         ('* |', 'c'), ('|/', ''), ('* ', 'd'))
        glyphs = probe.transform(rows)[0]
        def viewport(x, y, width, height):
            return {(px-x, py-y): value for (px, py), value in glyphs.items()
                    if x <= px < x+width and y <= py < y+height}
        first = viewport(0, 0, 3, 4)
        viewport(2, 4, 2, 3)
        self.assertEqual(first, viewport(0, 0, 3, 4))
        self.assertEqual(glyphs, probe.transform(rows)[0])


if __name__ == '__main__':
    unittest.main()
