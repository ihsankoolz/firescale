//! `expo/entry`: the app's entry point exists. package.json `main` names
//! it; Expo Router's is `expo-router/entry` (a root layout in `app/` or
//! `src/app/`); with no `main`, Expo's own `expo/AppEntry` loads `App` from
//! the root.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;
use serde_json::Value;

use super::{any_exists, applies, create, finding, package_json, uses_typescript};

#[cfg(test)]
mod tests;

pub const ID: &str = "expo/entry";

pub struct Entry;

const EXTENSIONS: [&str; 4] = ["tsx", "ts", "jsx", "js"];

/// The blank TypeScript template's `index.ts` (SDK 58).
pub const INDEX: &str = "import { registerRootComponent } from 'expo';\n\nimport App from './App';\n\n// registerRootComponent calls AppRegistry.registerComponent('main', () => App);\n// It also ensures that whether you load the app in Expo Go or in a native build,\n// the environment is set up appropriately\nregisterRootComponent(App);\n";

/// The blank TypeScript template's `App.tsx` (SDK 58).
pub const APP: &str = "import { StatusBar } from 'expo-status-bar';\nimport { StyleSheet, Text, View } from 'react-native';\n\nexport default function App() {\n  return (\n    <View style={styles.container}>\n      <Text>Open up App.tsx to start working on your app!</Text>\n      <StatusBar style=\"auto\" />\n    </View>\n  );\n}\n\nconst styles = StyleSheet.create({\n  container: {\n    flex: 1,\n    backgroundColor: '#fff',\n    alignItems: 'center',\n    justifyContent: 'center',\n  },\n});\n";

/// The same app in JavaScript, for an app without TypeScript.
pub const APP_JS: &str = "import { StatusBar } from 'expo-status-bar';\nimport { StyleSheet, Text, View } from 'react-native';\n\nexport default function App() {\n  return (\n    <View style={styles.container}>\n      <Text>Open up App.js to start working on your app!</Text>\n      <StatusBar style=\"auto\" />\n    </View>\n  );\n}\n\nconst styles = StyleSheet.create({\n  container: {\n    flex: 1,\n    backgroundColor: '#fff',\n    alignItems: 'center',\n    justifyContent: 'center',\n  },\n});\n";

/// A module path resolved as Metro would: as it is, or with an extension.
fn resolves(ctx: &RuleContext, path: &str) -> bool {
    let path = path.trim_start_matches("./");
    let file = |p: &str| {
        p.split('/')
            .fold(ctx.root().to_path_buf(), |dir, part| dir.join(part))
            .is_file()
    };
    file(path)
        || EXTENSIONS.iter().any(|ext| file(&format!("{path}.{ext}")))
        || EXTENSIONS
            .iter()
            .any(|ext| file(&format!("{path}/index.{ext}")))
}

fn has_app(ctx: &RuleContext) -> bool {
    resolves(ctx, "App")
}

/// What is wrong with the entry, if anything.
#[derive(Debug, PartialEq, Eq)]
enum Problem {
    /// `main: "index.ts"` (or none) with nothing to load: a new app.
    NewApp,
    /// `main` names a file that is missing, but App is there.
    MissingIndex,
    NoRouterLayout,
    Missing(String),
}

fn problem(ctx: &RuleContext, json: &Value) -> Option<Problem> {
    match json.get("main").and_then(Value::as_str) {
        Some("expo-router/entry") => {
            let layout = ["app/_layout", "src/app/_layout"].iter().any(|p| {
                EXTENSIONS
                    .iter()
                    .any(|ext| any_exists(ctx, &[&format!("{p}.{ext}")]))
            });
            (!layout).then_some(Problem::NoRouterLayout)
        }
        Some(main) if main.starts_with('.') || !main.contains('/') || main.starts_with("src/") => {
            if resolves(ctx, main) {
                None
            } else if main.trim_start_matches("./") == "index.ts" {
                Some(if has_app(ctx) {
                    Problem::MissingIndex
                } else {
                    Problem::NewApp
                })
            } else {
                Some(Problem::Missing(main.to_owned()))
            }
        }
        // A package's own entry (`expo-router/entry`'s kin): Node resolves it.
        Some(_) => None,
        None => (!has_app(ctx)).then_some(Problem::NewApp),
    }
}

impl BuiltinRule for Entry {
    fn meta(&self) -> RuleMeta {
        let mut inputs = vec!["package.json".to_owned()];
        for stem in ["App", "index", "app/_layout", "src/app/_layout"] {
            for ext in EXTENSIONS {
                inputs.push(format!("{stem}.{ext}"));
            }
        }
        inputs.push("**/*.{ts,tsx,js,jsx}".to_owned());
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::High,
            category: Category::Reliability,
            description: "The app's entry point exists: package.json main, Expo Router's root layout, or App at the root.".to_owned(),
            inputs,
            has_fix: true,
            after: vec![super::package_json::ID.to_owned()],
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        if !applies(ctx)? {
            return Ok(Vec::new());
        }
        // No package.json yet: expo/package-json writes one, and the next
        // pass of the fix loop comes back here.
        let Ok(Some(json)) = package_json(ctx)? else {
            return Ok(Vec::new());
        };
        Ok(match problem(ctx, &json) {
            None => Vec::new(),
            Some(Problem::NewApp) => vec![finding(
                "App.tsx",
                "The app has no entry point to load. `fscale sync` creates App and, when package.json's main is index.ts, the index.ts that registers it, as Expo's blank template does.".to_owned(),
                Fixable::Deterministic,
            )],
            Some(Problem::MissingIndex) => vec![finding(
                "index.ts",
                "package.json's main is index.ts, which does not exist. `fscale sync` creates it, registering App as Expo's template does.".to_owned(),
                Fixable::Deterministic,
            )],
            Some(Problem::NoRouterLayout) => vec![finding(
                "app/_layout.tsx",
                "package.json's main is expo-router/entry, but there is no root layout (app/_layout or src/app/_layout). Expo Router needs one: create it as Expo Router's docs describe.".to_owned(),
                Fixable::Agent,
            )],
            Some(Problem::Missing(main)) => vec![finding(
                "package.json",
                format!("package.json's main is {main}, which does not exist. Point main at the app's entry file, or create it."),
                Fixable::Agent,
            )],
        })
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        if !applies(ctx)? {
            return Ok(result);
        }
        let Ok(Some(json)) = package_json(ctx)? else {
            return Ok(result);
        };
        let ts = uses_typescript(ctx)?;
        let index_main = json.get("main").and_then(Value::as_str).is_some();
        match problem(ctx, &json) {
            Some(Problem::NewApp) => {
                if ts {
                    create(ctx, "App.tsx", APP, &mut result)?;
                } else {
                    create(ctx, "App.js", APP_JS, &mut result)?;
                }
                if index_main {
                    create(ctx, "index.ts", INDEX, &mut result)?;
                }
            }
            Some(Problem::MissingIndex) => create(ctx, "index.ts", INDEX, &mut result)?,
            _ => {}
        }
        Ok(result)
    }
}
