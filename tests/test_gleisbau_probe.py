"""Independent checks of the Unicode port oracle used for the library trial."""
import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts/experiments'))
import gleisbau_probe as probe


def graph(lines, positions):
    return dict(status='rendered', lines=lines,
                positions=[dict(oid=oid, column=x, row=y) for oid, x, y in positions])


def commits(mapping):
    return [dict(oid=oid, parents=parents) for oid, parents in mapping.items()]


class GleisbauOracleTests(unittest.TestCase):
    def test_linear_and_open_tail(self):
        output=graph(['●','●','│'], [('a',0,0),('b',0,1)])
        self.assertEqual(probe.oracle(output, commits({'a':['b'],'b':['outside']})), [])
        self.assertTrue(probe.oracle(output, commits({'a':['b'],'b':[]})))

    def test_crossing_does_not_join(self):
        output=graph(['● ●  ','│ │  ','╰─┼─╮','  │ │','  ● ●'],
                     [('a',0,0),('b',2,0),('d',2,4),('c',4,4)])
        expected=commits({'a':['c'],'b':['d'],'c':[],'d':[]})
        self.assertEqual(probe.oracle(output,expected), [])
        output['lines'][2]='╰─┬─╮'
        self.assertTrue(probe.oracle(output,expected))

    def test_arrow_prevents_backtracking_into_merge_child(self):
        output=graph(['●   ','│   ','├─>○','●  │','│  ●','●  │','├──╯','●   '],
                     [('a',0,0),('merge',3,2),('b',0,3),('c',3,4),('d',0,5),('root',0,7)])
        expected=commits({'a':['b'],'merge':['b','c'],'b':['d'],'c':['root'],'d':['root'],'root':[]})
        self.assertEqual(probe.oracle(output,expected), [])

    def test_wrong_parent_is_rejected(self):
        self.assertTrue(probe.oracle(graph(['●','●'],[('a',0,0),('b',0,1)]), commits({'a':[],'b':[]})))

    def test_missing_parent_is_rejected(self):
        self.assertTrue(probe.oracle(graph(['●  ','  ●'],[('a',0,0),('b',2,1)]), commits({'a':['b'],'b':[]})))

    def test_bad_row_map_and_glyph_are_rejected(self):
        self.assertTrue(probe.oracle(graph(['●'],[('a',2,0)]), commits({'a':[]})))
        self.assertTrue(probe.oracle(graph(['●','?'],[('a',0,0)]), commits({'a':[]})))

    def test_dangling_extra_branch_is_rejected_even_when_parent_is_correct(self):
        output=graph(['●  ', '├─╮', '● │', '   '], [('a',0,0),('b',0,2)])
        self.assertTrue(probe.oracle(output, commits({'a':['b'],'b':[]})))

    def test_duplicate_mapping_entry_is_rejected(self):
        output=graph(['●'], [('a',0,0),('a',0,0)])
        self.assertTrue(probe.oracle(output, commits({'a':[]})))

    def test_256_color_is_typed(self):
        self.assertEqual(probe.cells('\x1b[38;5;6m│\x1b[0m '), [('│',6),(' ',None)])
        with self.assertRaises(ValueError):
            probe.cells('\x1b[1m●')


if __name__=='__main__':
    unittest.main()
