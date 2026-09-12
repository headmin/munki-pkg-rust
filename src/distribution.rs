//! Distribution XML generation for multi-component packages
//!
//! Generates the distribution.xml file required by `productbuild --distribution`
//! to combine multiple component packages into a single distribution package.

use crate::config::BundleInfo;

/// Metadata for a built component package
pub struct ComponentPackage {
    /// Component name (used as choice id)
    pub name: String,
    /// Package identifier from the component's build-info
    pub identifier: String,
    /// Package version from the component's build-info
    pub version: String,
    /// Filename of the built .pkg (e.g. "dialog-1.0.pkg")
    pub pkg_filename: String,
}

/// Generate distribution.xml content from bundle info and built components.
///
/// Produces an installer-gui-script with:
/// - Non-customizable choices (all components always install)
/// - OS version gating via volume-check
/// - Root volume only installation
/// - Components ordered as listed in bundle-info
pub fn generate_distribution_xml(
    bundle_info: &BundleInfo,
    components: &[ComponentPackage],
) -> String {
    let mut xml = String::new();

    xml.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    xml.push_str("<installer-gui-script minSpecVersion=\"2\">\n");
    xml.push_str(&format!(
        "    <title>{}</title>\n",
        escape_xml(&bundle_info.name)
    ));
    xml.push_str(
        "    <options customize=\"never\" require-scripts=\"false\" rootVolumeOnly=\"true\"/>\n",
    );

    // Volume check for minimum OS version
    if let Some(ref min_os) = bundle_info.min_os_version {
        xml.push_str("    <volume-check>\n");
        xml.push_str("        <allowed-os-versions>\n");
        xml.push_str(&format!(
            "            <os-version min=\"{}\"/>\n",
            escape_xml(min_os)
        ));
        xml.push_str("        </allowed-os-versions>\n");
        xml.push_str("    </volume-check>\n");
    }

    // Choices outline — defines install order
    xml.push_str("    <choices-outline>\n");
    for comp in components {
        xml.push_str(&format!(
            "        <line choice=\"{}\"/>\n",
            escape_xml(&comp.name)
        ));
    }
    xml.push_str("    </choices-outline>\n");

    // Choice + pkg-ref for each component
    for comp in components {
        xml.push_str(&format!(
            "    <choice id=\"{}\" visible=\"false\">\n",
            escape_xml(&comp.name)
        ));
        xml.push_str(&format!(
            "        <pkg-ref id=\"{}\"/>\n",
            escape_xml(&comp.identifier)
        ));
        xml.push_str("    </choice>\n");
        xml.push_str(&format!(
            "    <pkg-ref id=\"{}\" version=\"{}\">{}</pkg-ref>\n",
            escape_xml(&comp.identifier),
            escape_xml(&comp.version),
            escape_xml(&comp.pkg_filename),
        ));
    }

    xml.push_str("</installer-gui-script>\n");
    xml
}

/// Minimal XML escaping for attribute/text values
fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BundleInfo, ComponentRef};

    #[test]
    fn test_generate_distribution_xml_basic() {
        let bundle = BundleInfo {
            name: "test-bundle-${version}.pkg".to_string(),
            version: "1.0.0".to_string(),
            identifier: "com.example.bundle".to_string(),
            min_os_version: Some("14.0".to_string()),
            install_location: "/".to_string(),
            components: vec![
                ComponentRef {
                    name: "dialog".to_string(),
                },
                ComponentRef {
                    name: "cli".to_string(),
                },
            ],
            signing_info: None,
            notarization_info: None,
        };

        let packages = vec![
            ComponentPackage {
                name: "dialog".to_string(),
                identifier: "com.swiftdialog.app".to_string(),
                version: "2.5.0".to_string(),
                pkg_filename: "dialog-2.5.0.pkg".to_string(),
            },
            ComponentPackage {
                name: "cli".to_string(),
                identifier: "com.ignitecli.pkg".to_string(),
                version: "1.0.0".to_string(),
                pkg_filename: "cli-1.0.0.pkg".to_string(),
            },
        ];

        let xml = generate_distribution_xml(&bundle, &packages);

        assert!(xml.contains("<?xml version=\"1.0\""));
        assert!(xml.contains("<title>test-bundle-${version}.pkg</title>"));
        assert!(xml.contains("customize=\"never\""));
        assert!(xml.contains("rootVolumeOnly=\"true\""));
        assert!(xml.contains("<os-version min=\"14.0\"/>"));
        assert!(xml.contains("<line choice=\"dialog\"/>"));
        assert!(xml.contains("<line choice=\"cli\"/>"));
        assert!(xml.contains("<choice id=\"dialog\" visible=\"false\">"));
        assert!(xml.contains("<pkg-ref id=\"com.swiftdialog.app\"/>"));
        assert!(xml.contains(
            "<pkg-ref id=\"com.swiftdialog.app\" version=\"2.5.0\">dialog-2.5.0.pkg</pkg-ref>"
        ));
        assert!(xml.contains(
            "<pkg-ref id=\"com.ignitecli.pkg\" version=\"1.0.0\">cli-1.0.0.pkg</pkg-ref>"
        ));
    }

    #[test]
    fn test_generate_distribution_xml_no_min_os() {
        let bundle = BundleInfo {
            name: "simple.pkg".to_string(),
            version: "1.0".to_string(),
            identifier: "com.example.simple".to_string(),
            min_os_version: None,
            install_location: "/".to_string(),
            components: vec![ComponentRef {
                name: "app".to_string(),
            }],
            signing_info: None,
            notarization_info: None,
        };

        let packages = vec![ComponentPackage {
            name: "app".to_string(),
            identifier: "com.example.app".to_string(),
            version: "1.0".to_string(),
            pkg_filename: "app-1.0.pkg".to_string(),
        }];

        let xml = generate_distribution_xml(&bundle, &packages);

        assert!(!xml.contains("<volume-check>"));
        assert!(!xml.contains("<os-version"));
        assert!(xml.contains("<choice id=\"app\""));
    }

    #[test]
    fn test_escape_xml() {
        assert_eq!(escape_xml("a & b"), "a &amp; b");
        assert_eq!(escape_xml("<script>"), "&lt;script&gt;");
        assert_eq!(escape_xml("say \"hi\""), "say &quot;hi&quot;");
        assert_eq!(escape_xml("normal"), "normal");
    }
}
