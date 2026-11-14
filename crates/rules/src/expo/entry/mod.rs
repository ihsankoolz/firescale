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

/// The extensions Expo tries (`getBareExtensions`, `@expo/config` 58).
const EXTENSIONS: [&str; 7] = ["tsx", "ts", "jsx", "js", "mjs", "cjs", "json"];

/// Platform variants Metro picks before the plain file.
const PLATFORMS: [&str; 3] = ["ios", "android", "native"];

/// The blank TypeScript template's `index.ts` (SDK 58).
pub const INDEX: &str = "import { registerRootComponent } from 'expo';\n\nimport App from './App';\n\n// registerRootComponent calls AppRegistry.registerComponent('main', () => App);\n// It also ensures that whether you load the app in Expo Go or in a native build,\n// the environment is set up appropriately\nregisterRootComponent(App);\n";

/// The blank TypeScript template's `App.tsx` (SDK 58).
pub const APP: &str = "import { StatusBar } from 'expo-status-bar';\nimport { StyleSheet, Text, View } from 'react-native';\n\nexport default function App() {\n  return (\n    <View style={styles.container}>\n      <Text>Open up App.tsx to start working on your app!</Text>\n      <StatusBar style=\"auto\" />\n    </View>\n  );\n}\n\nconst styles = StyleSheet.create({\n  container: {\n    flex: 1,\n    backgroundColor: '#fff',\n    alignItems: 'center',\n    justifyContent: 'center',\n  },\n});\n";

/// The same app in JavaScript, for an app without TypeScript.
pub const APP_JS: &str = "import { StatusBar } from 'expo-status-bar';\nimport { StyleSheet, Text, View } from 'react-native';\n\nexport default function App() {\n  return (\n    <View style={styles.container}>\n      <Text>Open up App.js to start working on your app!</Text>\n      <StatusBar style=\"auto\" />\n    </View>\n  );\n}\n\nconst styles = StyleSheet.create({\n  container: {\n    flex: 1,\n    backgroundColor: '#fff',\n    alignItems: 'center',\n    justifyContent: 'center',\n  },\n});\n";

/// A module path resolved as Expo and Metro would: as it is, with an
/// extension (a platform one too, `index.ios.js`), or as a folder's index.
fn resolves(ctx: &RuleContext, path: &str) -> bool {
    resolves_with(ctx, path, &EXTENSIONS)
}

/// A file at `path` below `root` whose name matches in letter case too:
/// on Windows and macOS `App.json` would otherwise find `app.json`.
fn exact_file(root: &std::path::Path, path: &str) -> bool {
    let full = path
        .split('/')
        .fold(root.to_path_buf(), |dir, part| dir.join(part));
    if !full.is_file() {
        return false;
    }
    let (Some(parent), Some(name)) = (full.parent(), full.file_name()) else {
        return false;
    };
    std::fs::read_dir(parent).is_ok_and(|entries| {
        entries
            .filter_map(std::result::Result::ok)
            .any(|e| e.file_name() == name)
    })
}

fn resolves_with(ctx: &RuleContext, path: &str, extensions: &[&str]) -> bool {
    let path = path.trim_start_matches("./").replace('\\', "/");
    let file = |p: &str| exact_file(ctx.root(), p);
    let with_ext = |stem: &str| {
        extensions.iter().any(|ext| {
            file(&format!("{stem}.{ext}"))
                || PLATFORMS
                    .iter()
                    .any(|platform| file(&format!("{stem}.{platform}.{ext}")))
        })
    };
    file(&path) || with_ext(&path) || with_ext(&format!("{path}/index"))
}

/// `App` as `expo/AppEntry` imports it: a code file, not `App.json`.
fn has_app(ctx: &RuleContext) -> bool {
    resolves_with(ctx, "App", &EXTENSIONS[..6])
}

/// The folder Expo Router reads routes from: a `root` set for the
/// expo-router plugin (or `extra.router.root`) in app.json, else `src/app`
/// when it exists, else `app`.
fn router_root(ctx: &RuleContext) -> Option<String> {
    let config = ctx
        .read("app.json")
        .ok()
        .flatten()
        .and_then(|t| serde_json::from_str::<Value>(t.trim_start_matches('\u{feff}')).ok());
    let expo = config.as_ref().and_then(|c| c.get("expo").or(Some(c)));
    let from_plugin = expo
        .and_then(|e| e.get("plugins"))
        .and_then(Value::as_array)
        .and_then(|plugins| {
            plugins.iter().find_map(|p| {
                let pair = p.as_array()?;
                (pair.first()?.as_str()? == "expo-router")
                    .then(|| pair.get(1)?.get("root")?.as_str().map(str::to_owned))?
            })
        });
    let from_extra = expo
        .and_then(|e| e.get("extra")?.get("router")?.get("root")?.as_str())
        .map(str::to_owned);
    from_plugin.or(from_extra)
}

/// True when the folder holds a file, at any depth (a route).
fn has_route(dir: &std::path::Path, depth: usize) -> bool {
    depth < 20
        && std::fs::read_dir(dir).is_ok_and(|entries| {
            entries.filter_map(std::result::Result::ok).any(|e| {
                let path = e.path();
                match e.file_type() {
                    Ok(t) if t.is_dir() => has_route(&path, depth + 1),
                    Ok(t) if t.is_file() => path
                        .extension()
                        .and_then(|x| x.to_str())
                        .is_some_and(|x| EXTENSIONS[..4].contains(&x)),
                    _ => false,
                }
            })
        })
}

/// True when `main` names a package's module rather than a file of the app.
fn is_package(json: &Value, main: &str) -> bool {
    let main = main.replace('\\', "/");
    if main.starts_with('.') || main.starts_with("node_modules/") {
        return main.trim_start_matches("./").starts_with("node_modules/");
    }
    let first = if main.starts_with('@') {
        main.splitn(3, '/').take(2).collect::<Vec<_>>().join("/")
    } else {
        main.split('/').next().unwrap_or("").to_owned()
    };
    main.contains('/') && (super::depends_on(json, &first) || first == "expo")
}

/// What is wrong with the entry, if anything.
#[derive(Debug, PartialEq, Eq)]
enum Problem {
    /// `main: "index.ts"` (or none) with nothing to load: a new app.
    NewApp,
    /// `main` names a file that is missing, but App is there.
    MissingIndex,
    NoRoutes,
    Missing(String),
}

fn problem(ctx: &RuleContext, json: &Value) -> Option<Problem> {
    match json.get("main").and_then(Value::as_str) {
        Some("expo-router/entry") => {
            // A root layout is optional (Expo Router adds a default one);
            // the routes folder must hold a route. A dynamic config may
            // move it where this cannot read.
            if any_exists(ctx, &super::app_config::DYNAMIC) {
                return None;
            }
            let roots: Vec<String> = match router_root(ctx) {
                Some(root) => vec![root],
                None => vec!["src/app".to_owned(), "app".to_owned()],
            };
            let found = roots.iter().any(|root| {
                let dir = root
                    .trim_start_matches("./")
                    .split('/')
                    .fold(ctx.root().to_path_buf(), |dir, part| dir.join(part));
                has_route(&dir, 0)
            });
            (!found).then_some(Problem::NoRoutes)
        }
        Some(main) if is_package(json, main) => None,
        Some(main) => {
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
        // No main: Expo tries `index` at the root, then `expo/AppEntry`,
        // which loads `App` (`resolveEntryPoint`, @expo/config 58).
        None => (!resolves(ctx, "index") && !has_app(ctx)).then_some(Problem::NewApp),
    }
}

impl BuiltinRule for Entry {
    fn meta(&self) -> RuleMeta {
        let mut inputs = vec!["package.json".to_owned()];
        inputs.extend([
            "app.json".to_owned(),
            "**/*.{ts,tsx,js,jsx,mjs,cjs,json}".to_owned(),
        ]);
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::High,
            category: Category::Reliability,
            description: "The app's entry point exists: package.json main, a route for Expo Router, or index or App at the root.".to_owned(),
            inputs: super::inputs(&inputs.iter().map(String::as_str).collect::<Vec<_>>()),
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
            Some(Problem::NoRoutes) => vec![finding(
                "app/index.tsx",
                "package.json's main is expo-router/entry, but the routes folder (src/app or app, or the root set for expo-router in app.json) holds no route. Add a first route, such as app/index.tsx, as Expo Router's docs describe.".to_owned(),
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
