# -*- coding: utf-8 -*-
"""Structural validation of the generated report.

必须给出**退出码**。早先的版本只把结论打印出来、永远退出 0，
于是任何串在后面的流水线看到的都是"成功"——包括 `tables: 0   inconsistent: 0` 这种
"报告里一张表都没有"的情况。判据在零个样本上是空洞成立的，所以下面把
"一个都没有"单独判为失败。
"""
import re
import sys
from html.parser import HTMLParser

PATH = (sys.argv[1] if len(sys.argv) > 1
        else r'A:\Ubuntu\测试\hls-downloader\outputs\v7-ui-audit-2026-09-14.html')
src = open(PATH, encoding='utf-8').read()

placeholders = re.findall(r'@@[A-Z_]+@@', src)
n_images = len(re.findall(r'<img ', src))
n_sections = len(re.findall(r'<h2>', src))
h1 = re.search(r'<h1>(.*?)</h1>', src, re.S)

print('bytes       :', len(src.encode('utf-8')))
print('placeholders:', placeholders or 'none')
print('images      :', n_images)
print('h1          :', h1.group(1) if h1 else '(none)')
print('h2 sections :', n_sections)


class T(HTMLParser):
    def __init__(self):
        super().__init__()
        self.tables, self.cur, self.row = [], None, None

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if tag == 'table':
            self.cur = []
        elif tag == 'tr' and self.cur is not None:
            self.row = []
        elif tag in ('td', 'th') and self.row is not None:
            self.row.append((int(a.get('colspan', 1)), int(a.get('rowspan', 1))))

    def handle_endtag(self, tag):
        if tag == 'tr' and self.row is not None:
            self.cur.append(self.row)
            self.row = None
        elif tag == 'table' and self.cur is not None:
            self.tables.append(self.cur)
            self.cur = None


p = T()
p.feed(src)


def grid(rows):
    """Occupy a real (row, col) matrix honouring colspan AND rowspan."""
    taken = set()
    widths = []
    for r, row in enumerate(rows):
        c = 0
        for cspan, rspan in row:
            while (r, c) in taken:
                c += 1
            for dr in range(rspan):
                for dc in range(cspan):
                    taken.add((r + dr, c + dc))
            c += cspan
        widths.append(len([1 for (rr, _cc) in taken if rr == r]))
    return widths


problems = []
if placeholders:
    problems.append('%d 个未替换的占位符' % len(placeholders))
if n_images == 0:
    problems.append('报告里一张图都没有 —— 图片内嵌环节失效，不是"零差异"')
if n_sections == 0:
    problems.append('报告里没有 <h2> 小节')
if h1 is None:
    problems.append('报告里没有 <h1>')

bad = 0
empty_tables = 0
for i, rows in enumerate(p.tables, 1):
    if not rows:
        # 空表以前被 continue 掉，既不算坏也不计入——同样是"静默跳过"。
        empty_tables += 1
        bad += 1
        print('  table %d is EMPTY' % i)
        continue
    w = grid(rows)
    if len(set(w)) != 1:
        bad += 1
        print('  table %d inconsistent: %s' % (i, sorted(set(w))))
print('tables      : %d   inconsistent: %d   empty: %d'
      % (len(p.tables), bad - empty_tables, empty_tables))

if len(p.tables) == 0:
    problems.append('报告里一张表都没有 —— 表格生成环节失效，不是"零不一致"')
if bad:
    problems.append('%d 张表结构不一致' % bad)

for p_ in problems:
    print('!!', p_)
print('verdict     :', 'PASS' if not problems else 'FAIL')
sys.exit(1 if problems else 0)
