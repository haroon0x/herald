"""The shipped classifier prompt, extracted from the Rust source.

Running the eval against this file proves the port is faithful: if the Rust
`format!` string and the Python mirror differ, the recorded 31/42 would not
transfer to the binary.
"""

import re
from pathlib import Path

RUST = Path(__file__).resolve().parents[2] / "crates/herald-core/src/intent.rs"


def load() -> str:
    source = RUST.read_text()
    start = source.index('r#"You classify one user request')
    end = source.index('"#,', start)
    body = source[start + 3 : end]
    operations = re.search(r"const OPERATIONS: \[&str; 7\] = \[(.*?)\];", source, re.S)
    names = re.findall(r'"([a-z_]+)"', operations.group(1))
    body = body.replace("{OPERATIONS:?}", repr(names))
    return body.replace("{{", "{").replace("}}", "}")


SHIPPED_SYSTEM = load()

if __name__ == "__main__":
    print(SHIPPED_SYSTEM)
