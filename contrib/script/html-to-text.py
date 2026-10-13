import sys
from html.parser import HTMLParser
from pathlib import Path

parser = HTMLParser()
parser.handle_data = lambda text: sys.stdout.write(text)
parser.feed(Path(sys.argv[1]).read_text(encoding="utf-8"))
