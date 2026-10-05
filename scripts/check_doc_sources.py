"""Check current owner docs' local links and explicit Python symbols; never grade historical audit prose.

Use `<!-- source: ferb/module.py#Class.method -->` or `// source: ferb/module.py#function` for symbols.
Run from any directory: `python scripts/check_doc_sources.py`. No imports of application modules are needed.
"""

import argparse
import ast
from pathlib import Path
import re
from urllib.parse import unquote

DEFAULT_DOCS = ('README.md', 'ARCHITECTURE.md', 'docs/OWNER_GUIDE.md', 'docs/RPC.md',
                'docs/CAPABILITIES.md', 'docs/OWNER_ACCEPTANCE.md', 'docs/walkthrough/model.js')
REFERENCE = re.compile(r'source:\s*([\w./-]+\.py)(?:#([A-Za-z_][\w.]*))?')
CODE_PATH = re.compile(r'\bferb/[\w./-]+\.(?:py|md|sql)\b')
LINK = re.compile(r'\[[^\]\n]*\]\(([^)\n]+)\)')


def symbols(path: Path) -> set[str]:
    result: set[str] = set()
    def walk(nodes, prefix=''):
        for node in nodes:
            if isinstance(node, (ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)):
                name = prefix + node.name
                result.add(name)
                walk(node.body, name + '.')
    walk(ast.parse(path.read_text(encoding='utf-8')).body)
    return result


def check(root: Path, documents: list[str]) -> list[str]:
    errors: list[str] = []
    for name in documents:
        path = root / name
        if not path.is_file():
            errors.append(f'{name}: document does not exist')
            continue
        text = path.read_text(encoding='utf-8')
        for line_number, line in enumerate(text.splitlines(), 1):
            prefix = f'{name}:{line_number}'
            for code in CODE_PATH.findall(line):
                if not (root / code).is_file():
                    errors.append(f'{prefix}: source file does not exist: {code}')
            for code, symbol in REFERENCE.findall(line):
                source = root / code
                if source.is_file() and symbol and symbol not in symbols(source):
                    errors.append(f'{prefix}: Python symbol does not exist: {code}#{symbol}')
            if path.suffix == '.md':
                for raw in LINK.findall(line):
                    destination = raw.strip().strip('<>').split(' "', 1)[0]
                    if not destination or destination.startswith('#') or re.match(r'[a-zA-Z]+:', destination):
                        continue
                    target = unquote(destination.split('#', 1)[0])
                    if not (path.parent / target).exists():
                        errors.append(f'{prefix}: local link does not exist: {target}')
    return sorted(set(errors))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('documents', nargs='*', default=list(DEFAULT_DOCS))
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    errors = check(root, args.documents)
    for error in errors:
        print(error)
    if not errors:
        print(f'Documentation source references passed ({len(args.documents)} documents).')
    return bool(errors)


if __name__ == '__main__':
    raise SystemExit(main())
