// Quick test binary to generate an actual PDF from the Typst pipeline.
// Run with: cargo run -p kyomi-agent --example generate_pdf

use std::collections::HashMap;

fn main() {
    // Simulate a dashboard markdown with various elements
    let markdown = r#"# Monthly Revenue Report

## Key Metrics

Revenue grew **20%** month-over-month, driven by enterprise contracts.

## Summary Table

| Region | Revenue | Growth |
|--------|---------|--------|
| North America | $1.2M | +15% |
| Europe | $800K | +22% |
| Asia Pacific | $450K | +31% |

## Highlights

- Enterprise deals closed: **12**
- New customers: *47*
- Churn rate: `1.2%`

### Action Items

1. Expand APAC sales team
2. Launch Q2 marketing campaign
3. Review pricing for mid-market tier

---

*Report generated automatically by Kyomi.*
"#;

    // Convert markdown to Typst
    let typst_body = kyomi_agent::markdown_to_typst::markdown_to_typst(markdown);

    // Wrap with document template
    let typst_doc = kyomi_agent::pdf_typst::wrap_document("Monthly Revenue Report", &typst_body);

    println!("=== Generated Typst source ===");
    println!("{typst_doc}");
    println!("==============================\n");

    // Generate PDF
    let images = HashMap::new();
    let pdf_bytes = kyomi_agent::pdf_typst::generate_pdf(&typst_doc, &images)
        .expect("PDF generation failed");

    let output_path = "test_output.pdf";
    std::fs::write(output_path, &pdf_bytes).expect("Failed to write PDF");
    println!("PDF written to: {output_path} ({} bytes)", pdf_bytes.len());
}
