//! Small documents for viewer tests.

/// A valid PDF with one Helvetica text line and a blue square per page.
pub(crate) fn pdf(pages: &[&str]) -> Vec<u8> {
    assert!(!pages.is_empty(), "a PDF needs a page");
    let kids: Vec<String> = (0..pages.len())
        .map(|i| format!("{} 0 R", 4 + 2 * i))
        .collect();
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            pages.len()
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
    ];
    for (i, text) in pages.iter().enumerate() {
        assert!(text.chars().all(|c| c.is_ascii_alphanumeric() || c == ' '));
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] \
             /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
            5 + 2 * i
        ));
        let stream =
            format!("0.1 0.3 0.9 rg 40 40 220 150 re f BT /F1 18 Tf 40 240 Td ({text}) Tj ET");
        objects.push(format!(
            "<< /Length {} >>\nstream\n{stream}\nendstream",
            stream.len()
        ));
    }
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// Headings, a list, a table, code, a link, and content that must never show.
pub(crate) const HTML: &str = r#"<!DOCTYPE html>
<html><head><title>Hidden title</title>
<style>body { color: red } .SECRET_STYLE {}</style>
<script>document.write("SECRET_SCRIPT")</script></head>
<body>
<h1>Quarterly report</h1>
<p>Revenue grew <strong>twelve</strong> percent; see <a href="https://example.com/details">the details</a>.</p>
<h2>Highlights</h2>
<ul><li>First item</li><li>Second item</li></ul>
<table><tr><th>Region</th><th>Sales</th></tr><tr><td>North</td><td>42</td></tr></table>
<pre><code>fn main() {}</code></pre>
<script>alert("SECRET_SCRIPT_TWO")</script>
</body></html>
"#;
