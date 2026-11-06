"""Parse only; never import or execute the supplied source. Output line IDs only."""
import ast
import json
import re
import sys


def key(node):
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        parent = key(node.value)
        return parent + "." + node.attr if parent else None
    return None


def literal(node):
    return node.value if isinstance(node, ast.Constant) and isinstance(node.value, str) else None


def local_segment(node):
    value = literal(node)
    return value is not None and value and not value.startswith("/") and "\\" not in value and all(part not in ("", ".", "..") for part in value.split("/"))


def local_path(node, roots):
    if key(node) in roots:
        return True
    if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Div):
        return local_path(node.left, roots) and local_segment(node.right)
    return isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr == "joinpath" and local_path(node.func.value, roots) and node.args and all(local_segment(arg) for arg in node.args)


TEMPORARY_CONSTRUCTORS = set()
PATH_CONSTRUCTORS = set()


def fixture_roots(body, initial=()):
    roots = set(initial)
    handles = set()
    for statement in body:
        if not isinstance(statement, (ast.Assign, ast.AnnAssign)):
            continue
        targets = statement.targets if isinstance(statement, ast.Assign) else [statement.target]
        value = statement.value
        for target in targets:
            name = key(target)
            if not name:
                continue
            roots.discard(name)
            handles.discard(name)
            if isinstance(value, ast.Call) and key(value.func) in TEMPORARY_CONSTRUCTORS and not value.args and not value.keywords:
                handles.add(name)
            elif isinstance(value, ast.Call) and key(value.func) in PATH_CONSTRUCTORS and len(value.args) == 1 and key(value.args[0]) in {handle + ".name" for handle in handles}:
                roots.add(name)
            elif local_path(value, roots):
                roots.add(name)
    return roots


def assertions(function):
    absent = []
    redacted = set()
    for node in ast.walk(function):
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and len(node.args) >= 2:
            value = literal(node.args[0])
            if value is None:
                continue
            target = ast.dump(node.args[1], include_attributes=False)
            if node.func.attr == "assertNotIn" and len(value) >= 8:
                absent.append((value, target))
            if node.func.attr == "assertIn" and "redact" in value.lower():
                redacted.add(target)
        elif isinstance(node, ast.Assert) and isinstance(node.test, ast.Compare) and len(node.test.ops) == 1:
            value = literal(node.test.left)
            if value is None:
                continue
            target = ast.dump(node.test.comparators[0], include_attributes=False)
            if isinstance(node.test.ops[0], ast.NotIn) and len(value) >= 8:
                absent.append((value, target))
            if isinstance(node.test.ops[0], ast.In) and "redact" in value.lower():
                redacted.add(target)
    return [(value, target) for value, target in absent if target in redacted]


def analyze(function, initial, allowed):
    if not isinstance(function, (ast.FunctionDef, ast.AsyncFunctionDef)) or not function.name.startswith("test_"):
        return
    roots = set(initial)
    roots.update(arg.arg for arg in function.args.args if arg.arg == "tmp_path")
    checks = assertions(function)
    if not roots or not checks:
        return
    for statement in function.body:
        if isinstance(statement, (ast.Assign, ast.AnnAssign)):
            roots = fixture_roots([statement], roots)
        for node in ast.walk(statement):
            if not isinstance(node, ast.Call) or not isinstance(node.func, ast.Attribute) or node.func.attr != "write_text" or not node.args:
                continue
            value = literal(node.args[0])
            if value is None or not local_path(node.func.value, roots):
                continue
            if any(needle in value for needle, _ in checks):
                allowed.update(range(node.args[0].lineno, node.args[0].end_lineno + 1))


def main():
    source = sys.stdin.read(2 * 1024 * 1024 + 1)
    if len(source.encode("utf-8")) > 2 * 1024 * 1024:
        return []
    tree = ast.parse(source)
    for statement in tree.body:
        if isinstance(statement, ast.Import):
            for name in statement.names:
                if name.name == "tempfile":
                    TEMPORARY_CONSTRUCTORS.add((name.asname or name.name) + ".TemporaryDirectory")
                if name.name == "pathlib":
                    PATH_CONSTRUCTORS.add((name.asname or name.name) + ".Path")
        elif isinstance(statement, ast.ImportFrom):
            for name in statement.names:
                if statement.module == "tempfile" and name.name == "TemporaryDirectory":
                    TEMPORARY_CONSTRUCTORS.add(name.asname or name.name)
                if statement.module == "pathlib" and name.name == "Path":
                    PATH_CONSTRUCTORS.add(name.asname or name.name)
    if sum(1 for _ in ast.walk(tree)) > 200000:
        return []
    # Recheck decoded literal data too; source escape spelling is not a bypass.
    for node in ast.walk(tree):
        if isinstance(node, ast.Constant) and isinstance(node.value, (str, bytes)):
            value = node.value.decode("ascii", "ignore") if isinstance(node.value, bytes) else node.value
            if re.search(r"(?:sk-|gh[pousr]_)[A-Za-z0-9]{25,}", value):
                return []
            for header in re.finditer(r"-----BEGIN (?:RSA |EC |DSA |OPENSSH |ENCRYPTED )?PRIVATE KEY-----", value):
                body = value[header.end():]
                if re.search(r"[A-Za-z0-9+/=]{48,}", body):
                    return []
    allowed = set()
    if len(sys.argv) > 1 and sys.argv[1] == "detectors":
        compilers = set()
        for statement in tree.body:
            if isinstance(statement, ast.Import):
                for name in statement.names:
                    if name.name == "re":
                        compilers.add((name.asname or name.name) + ".compile")
            elif isinstance(statement, ast.ImportFrom) and statement.module == "re":
                compilers.update(name.asname or name.name for name in statement.names if name.name == "compile")
        spans = []
        for node in ast.walk(tree):
            if isinstance(node, ast.Call) and key(node.func) in compilers and node.args and isinstance(node.args[0], ast.Constant) and isinstance(node.args[0].value, (str, bytes)):
                value = node.args[0].value
                value = value.decode("ascii", "ignore") if isinstance(value, bytes) else value
                if "PRIVATE KEY-----" in value:
                    literal_node = node.args[0]
                    spans.append((literal_node.lineno, literal_node.col_offset, literal_node.end_lineno, literal_node.end_col_offset))
                    allowed.update(range(literal_node.lineno, literal_node.end_lineno + 1))
        # Line-only output is safe only when every occurrence on that line is
        # actually contained in one of the approved regex literal AST spans.
        for number, raw_line in enumerate(source.splitlines(), 1):
            encoded = raw_line.encode("utf-8")
            positions = []
            start = 0
            while True:
                position = encoded.find(b"PRIVATE KEY-----", start)
                if position < 0:
                    break
                positions.append(position)
                start = position + 1
            if positions and any(not any((number > first or position >= column) and (number < last or position + 16 <= end_column) and first <= number <= last for first, column, last, end_column in spans) for position in positions):
                allowed.discard(number)
        return sorted(allowed)[:1000]
    for node in tree.body:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            analyze(node, (), allowed)
        elif isinstance(node, ast.ClassDef):
            setup = next((item for item in node.body if isinstance(item, ast.FunctionDef) and item.name == "setUp"), None)
            roots = fixture_roots(setup.body) if setup else set()
            for method in node.body:
                analyze(method, roots, allowed)
    return sorted(allowed)[:1000]


try:
    result = main()
except Exception:
    result = []
print(json.dumps(result))
