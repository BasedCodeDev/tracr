"""Preview serialization must preserve the cached adapter's exact cell text."""
import sys
from pathlib import Path
import unittest
import xml.etree.ElementTree as ET
sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'scripts/experiments'))
from gleisbau_acceptance import svg

class PreviewTests(unittest.TestCase):
    def test_svg_does_not_insert_whitespace_at_colour_boundaries(self):
        view={'request': {'width':32}, 'rows': [[{'text':'│ ', 'color':6}, {'text':'● e\u0301 界 <subject>', 'color':None}]]}
        document=ET.fromstring(svg(view,'actual output'))
        text=document.findall('{http://www.w3.org/2000/svg}text')[-1]
        self.assertEqual(''.join(text.itertext()), '│ ● e\u0301 界 <subject>')

if __name__=='__main__': unittest.main()
