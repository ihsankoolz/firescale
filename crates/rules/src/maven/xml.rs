//! A small reader of a POM's element structure (ADR 0054, after review).
//!
//! The pack edits `pom.xml` in place, so it must know exactly where the
//! project's own `<properties>` are, and not take a plugin's
//! `<configuration><properties>` or `<dependencies>` for the project's. This
//! reads the elements of a document with their paths (`project/properties`)
//! and byte ranges, skipping comments, CDATA sections, processing
//! instructions and the doctype. It is not a full XML parser: it reads what
//! a POM is made of, and returns `None` for a document it cannot follow, so
//! no fix is ever made on a guess.

/// One element of the document.
#[derive(Debug, Clone)]
pub struct Element {
    /// The names from the root to this element (`project`, `properties`),
    /// as written: Maven reads a POM's elements without a namespace prefix,
    /// and rejects `pom:project` (run here), so a prefixed element is not
    /// the project's.
    pub path: Vec<String>,
    /// Where its start tag begins (`<`) and ends (after `>`).
    pub start: usize,
    pub open_end: usize,
    /// Where its end tag begins (`</`), or `open_end` when it closes itself.
    pub close_start: usize,
    /// After its end tag.
    pub end: usize,
}

impl Element {
    /// Its path, as one string: `project/properties`.
    #[must_use]
    pub fn at(&self) -> String {
        self.path.join("/")
    }

    /// Its text, when it holds only text (trimmed).
    #[must_use]
    pub fn text<'d>(&self, document: &'d str) -> &'d str {
        document
            .get(self.open_end..self.close_start)
            .unwrap_or_default()
            .trim()
    }
}

/// The elements of `document`, in document order, or `None` when it is not
/// a document this reader can follow (an unclosed tag, comment or element).
#[must_use]
pub fn elements(document: &str) -> Option<Vec<Element>> {
    let bytes = document.as_bytes();
    let mut found: Vec<Element> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    let mut at = 0;
    while let Some(offset) = document[at..].find('<') {
        let start = at + offset;
        let rest = &document[start..];
        let skip_to = |end: &str| rest.find(end).map(|i| start + i + end.len());
        if rest.starts_with("<!--") {
            at = skip_to("-->")?;
        } else if rest.starts_with("<![CDATA[") {
            at = skip_to("]]>")?;
        } else if rest.starts_with("<?") {
            at = skip_to("?>")?;
        } else if rest.starts_with("<!") {
            at = skip_to(">")?;
        } else if rest.starts_with("</") {
            let close = skip_to(">")?;
            let name = local(document[start + 2..close - 1].trim());
            let index = open.pop()?;
            if found[index].path.last().map(String::as_str) != Some(name) {
                return None;
            }
            found[index].close_start = start;
            found[index].end = close;
            at = close;
        } else {
            // A start tag: its end is the first `>` outside a quoted value.
            let mut quote: Option<u8> = None;
            let mut end = None;
            for (i, byte) in bytes[start + 1..].iter().enumerate() {
                match (quote, *byte) {
                    (Some(q), b) if b == q => quote = None,
                    (Some(_), _) => {}
                    (None, b'"' | b'\'') => quote = Some(*byte),
                    (None, b'>') => {
                        end = Some(start + 1 + i + 1);
                        break;
                    }
                    _ => {}
                }
            }
            let open_end = end?;
            let inner = &document[start + 1..open_end - 1];
            let closes_itself = inner.ends_with('/');
            let name = local(
                inner
                    .trim_end_matches('/')
                    .split(|c: char| c.is_whitespace())
                    .next()
                    .unwrap_or_default(),
            );
            if name.is_empty() {
                return None;
            }
            let mut path: Vec<String> = open
                .last()
                .map(|parent| found[*parent].path.clone())
                .unwrap_or_default();
            path.push(name.to_owned());
            found.push(Element {
                path,
                start,
                open_end,
                close_start: open_end,
                end: open_end,
            });
            if !closes_itself {
                open.push(found.len() - 1);
            }
            at = open_end;
        }
    }
    open.is_empty().then_some(found)
}

/// An element's name as written.
fn local(name: &str) -> &str {
    name
}

/// The first element at `path` (`project/properties`).
#[must_use]
pub fn find<'e>(elements: &'e [Element], path: &str) -> Option<&'e Element> {
    elements.iter().find(|element| element.at() == path)
}

/// The text of the `artifactId` child of the plugin element `plugin`.
#[must_use]
pub fn plugin_artifact<'d>(elements: &[Element], plugin: &Element, document: &'d str) -> &'d str {
    let own = format!("{}/artifactId", plugin.at());
    elements
        .iter()
        .filter(|element| element.start > plugin.start && element.end <= plugin.end)
        .find(|element| element.at() == own)
        .map_or("", |element| element.text(document))
}

/// The children of `project/build/plugins/plugin` (and of
/// `pluginManagement`) whose plugin is `artifact`: their configuration
/// element names.
#[must_use]
pub fn plugin_settings(elements: &[Element], document: &str, artifact: &str) -> Vec<String> {
    let plugins = [
        "project/build/plugins/plugin",
        "project/build/pluginManagement/plugins/plugin",
    ];
    let mut found = Vec::new();
    for plugin in elements
        .iter()
        .filter(|e| plugins.contains(&e.at().as_str()))
    {
        if plugin_artifact(elements, plugin, document) != artifact {
            continue;
        }
        let configuration = format!("{}/configuration/", plugin.at());
        for element in elements
            .iter()
            .filter(|e| e.start > plugin.start && e.end <= plugin.end)
        {
            if let Some(name) = element.at().strip_prefix(&configuration) {
                if !name.contains('/') {
                    found.push(name.to_owned());
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_skip_comments_cdata_and_declarations() {
        let pom = "<?xml version=\"1.0\"?>\n<!-- <properties> -->\n<project a=\"x>y\">\n  <properties><a>1</a></properties>\n  <build><![CDATA[<build>]]><plugins><plugin><configuration><properties/></configuration></plugin></plugins></build>\n</project>\n";
        let elements = elements(pom).unwrap();
        let paths: Vec<String> = elements.iter().map(Element::at).collect();
        assert_eq!(
            paths,
            [
                "project",
                "project/properties",
                "project/properties/a",
                "project/build",
                "project/build/plugins",
                "project/build/plugins/plugin",
                "project/build/plugins/plugin/configuration",
                "project/build/plugins/plugin/configuration/properties",
            ]
        );
        assert_eq!(
            find(&elements, "project/properties/a").unwrap().text(pom),
            "1"
        );
    }

    #[test]
    fn a_document_it_cannot_follow_is_none() {
        assert!(elements("<project><a></project>").is_none());
        assert!(elements("<project><!-- open").is_none());
        assert!(elements("<project>").is_none());
        assert!(elements("<project><a\n").is_none());
    }

    #[test]
    fn prefixes_are_kept_and_non_ascii_text_is_read() {
        let pom = "<pom:project xmlns:pom=\"x\"><pom:name>caf\u{e9}\u{a0}</pom:name></pom:project>";
        let elements = elements(pom).unwrap();
        assert!(find(&elements, "project").is_none());
        assert_eq!(
            find(&elements, "pom:project/pom:name").unwrap().text(pom),
            "caf\u{e9}"
        );
    }

    #[test]
    fn plugin_settings_belong_to_their_plugin() {
        let pom = "<project><build><plugins>\
            <plugin><artifactId>maven-resources-plugin</artifactId><configuration><encoding>UTF-8</encoding></configuration></plugin>\
            <plugin><artifactId>maven-compiler-plugin</artifactId><configuration><release>17</release></configuration></plugin>\
            </plugins></build></project>";
        let elements = elements(pom).unwrap();
        assert_eq!(
            plugin_settings(&elements, pom, "maven-compiler-plugin"),
            ["release"]
        );
        assert_eq!(
            plugin_settings(&elements, pom, "maven-resources-plugin"),
            ["encoding"]
        );
    }
}
