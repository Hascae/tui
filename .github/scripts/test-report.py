"""Summarises JUnit XML results; fails when a test failed, was skipped or nothing ran."""

import sys
import xml.etree.ElementTree as ElementTree
from pathlib import Path

device, results = sys.argv[1], Path(sys.argv[2])
cases = []
for report in sorted(results.glob("**/TEST-*.xml")):
    for case in ElementTree.parse(report).getroot().iter("testcase"):
        problem = next((child for child in case if child.tag in ("failure", "error", "skipped")), None)
        cases.append((f"{case.get('classname')}.{case.get('name')}", float(case.get("time") or 0), problem))

for name, seconds, problem in cases:
    print(f"{'PASS' if problem is None else problem.tag.upper():8} {seconds:6.1f}s  {name}")
    if problem is not None and problem.tag != "skipped":
        print(problem.get("message") or "")
        print(problem.text or "")

failed = sum(problem is not None for _, _, problem in cases)
print(f"{device}: {len(cases)} tests, {failed} not passed")
if not cases or failed:
    print(f"::error::{device}: {failed} of {len(cases)} tests did not pass" if cases else f"::error::{device}: no results")
    sys.exit(1)
