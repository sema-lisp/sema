#!/usr/bin/env bash
#
# Generate the small PDF fixtures the PDF-stdlib tests read from
# crates/sema/tests/fixtures, via a self-contained Python heredoc (no external
# PDF libraries).
set -euo pipefail
FIXTURE_DIR="crates/sema/tests/fixtures"
mkdir -p "$FIXTURE_DIR"

python3 <<'PYEOF'
import sys

def make_pdf(text, path, title="Test Document", author="Sema Test Suite"):
    content = f'BT /F1 12 Tf 100 700 Td ({text}) Tj ET'
    content_bytes = content.encode()
    pdf = b'%PDF-1.4\n'
    offsets = []

    offsets.append(len(pdf))
    pdf += b'1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n'
    offsets.append(len(pdf))
    pdf += b'2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n'
    offsets.append(len(pdf))
    pdf += b'3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n'
    offsets.append(len(pdf))
    pdf += f'4 0 obj\n<< /Length {len(content_bytes)} >>\nstream\n'.encode() + content_bytes + b'\nendstream\nendobj\n'
    offsets.append(len(pdf))
    pdf += b'5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n'
    offsets.append(len(pdf))
    pdf += f'6 0 obj\n<< /Title ({title}) /Author ({author}) >>\nendobj\n'.encode()

    xref_offset = len(pdf)
    pdf += b'xref\n'
    pdf += f'0 {len(offsets) + 1}\n'.encode()
    pdf += b'0000000000 65535 f \n'
    for off in offsets:
        pdf += f'{off:010d} 00000 n \n'.encode()
    pdf += b'trailer\n'
    pdf += f'<< /Size {len(offsets) + 1} /Root 1 0 R /Info 6 0 R >>\n'.encode()
    pdf += b'startxref\n'
    pdf += f'{xref_offset}\n'.encode()
    pdf += b'%%EOF\n'

    with open(path, 'wb') as f:
        f.write(pdf)
    print(f'  Created {path} ({len(pdf)} bytes)')

print('Generating PDF fixtures...')
make_pdf(
    'Invoice 2025-01-15 Acme Corp Total: 1234.56 USD Billed to: Test Customer',
    'crates/sema/tests/fixtures/sample-invoice.pdf'
)
make_pdf(
    'Meeting Notes - Q4 Planning Session - Internal Document - Not an invoice',
    'crates/sema/tests/fixtures/not-a-receipt.pdf',
    title='Meeting Notes',
    author='Internal'
)
print('Done.')
PYEOF
