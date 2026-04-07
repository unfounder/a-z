// ── Form detection from raw HTML ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FormField {
    pub name: String,
    pub field_type: String,
    pub value: String,
    pub placeholder: String,
}

#[derive(Debug, Clone)]
pub struct Form {
    pub action: String,
    pub method: String,
    pub fields: Vec<FormField>,
}

/// Extract all forms from raw HTML using a simple scan (no extra crate needed).
pub fn extract_forms(html: &str) -> Vec<Form> {
    let mut forms = Vec::new();
    let lower = html.to_lowercase();
    let mut search = lower.as_str();
    let mut html_pos = html; // parallel cursor for original case values

    while let Some(form_start) = search.find("<form") {
        search = &search[form_start + 5..];
        html_pos = &html_pos[form_start + 5..];

        let action = attr_value(search, "action").unwrap_or_default();
        let method = attr_value(search, "method")
            .unwrap_or_else(|| "GET".into())
            .to_uppercase();

        // Find matching </form>
        let form_end = search.find("</form").unwrap_or(search.len());
        let form_body = &search[..form_end];
        let form_body_orig = &html_pos[..form_end];

        let fields = extract_fields(form_body, form_body_orig);

        if !fields.is_empty() || !action.is_empty() {
            forms.push(Form {
                action,
                method,
                fields,
            });
        }

        if form_end < search.len() {
            search = &search[form_end + 7..];
            html_pos = &html_pos[form_end + 7..];
        } else {
            break;
        }
    }

    forms
}

fn extract_fields(lower_body: &str, orig_body: &str) -> Vec<FormField> {
    let mut fields = Vec::new();
    let mut s = lower_body;
    let mut o = orig_body;

    while let Some(pos) = s.find("<input").or_else(|| s.find("<textarea")) {
        let tag_end = s[pos..].find('>').map(|e| pos + e).unwrap_or(s.len());
        let tag_frag = &s[pos..tag_end];
        let tag_frag_orig = &o[pos..tag_end.min(o.len())];

        let name = attr_value(tag_frag, "name").unwrap_or_default();
        if !name.is_empty() {
            let field_type = attr_value(tag_frag, "type").unwrap_or_else(|| "text".into());
            let value = attr_value(tag_frag_orig, "value").unwrap_or_default();
            let placeholder = attr_value(tag_frag_orig, "placeholder").unwrap_or_default();

            fields.push(FormField {
                name,
                field_type,
                value,
                placeholder,
            });
        }

        s = &s[tag_end + 1..];
        o = &o[tag_end.min(o.len() - 1) + 1..];
    }

    fields
}

fn attr_value(fragment: &str, attr: &str) -> Option<String> {
    // Try attr="value"
    let dq = format!("{}=\"", attr);
    if let Some(start) = fragment.find(&dq) {
        let rest = &fragment[start + dq.len()..];
        if let Some(end) = rest.find('"') {
            return Some(rest[..end].to_string());
        }
    }
    // Try attr='value'
    let sq = format!("{}='", attr);
    if let Some(start) = fragment.find(&sq) {
        let rest = &fragment[start + sq.len()..];
        if let Some(end) = rest.find('\'') {
            return Some(rest[..end].to_string());
        }
    }
    None
}
