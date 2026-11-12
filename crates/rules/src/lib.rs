//! Built-in rule packs. Every rule here is compiled into the `fscale` binary.

use firescale_core::rule::BuiltinPack;

#[path = "agent-ready/mod.rs"]
pub mod agent_ready;
pub mod core;
#[path = "group-project/mod.rs"]
pub mod group_project;
pub mod nextjs;

/// Every built-in pack, in the order they are listed to users.
#[must_use]
pub fn builtin_packs() -> Vec<BuiltinPack> {
    vec![
        core::pack(),
        agent_ready::pack(),
        group_project::pack(),
        nextjs::pack(),
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use firescale_core::fixloop::{plan_order, LoopRule};

    use super::builtin_packs;

    /// Every built-in rule, as the fix loop sees it.
    fn all_rules() -> Vec<LoopRule> {
        let mut rules: Vec<LoopRule> = builtin_packs()
            .iter()
            .flat_map(|pack| pack.rules.iter())
            .map(|rule| {
                let meta = rule.meta();
                LoopRule {
                    id: meta.id,
                    after: meta.after,
                }
            })
            .collect();
        rules.sort_by(|a, b| a.id.cmp(&b.id));
        rules
    }

    #[test]
    fn every_after_of_a_built_in_rule_names_a_built_in_rule_and_there_is_no_cycle() {
        let rules = all_rules();
        // Only built-in ids are known here, so an `after` that names anything
        // else, and any cycle, fails this test.
        let known: BTreeSet<String> = rules.iter().map(|rule| rule.id.clone()).collect();
        let order = plan_order(&rules, &known).unwrap();
        assert_eq!(order.len(), rules.len());
    }

    #[test]
    fn the_env_rule_is_fixed_after_the_rule_that_creates_gitignore() {
        let rules = all_rules();
        let env = rules
            .iter()
            .find(|rule| rule.id == "core/no-committed-env")
            .unwrap();
        assert_eq!(env.after, ["core/gitignore-basics"]);

        let known: BTreeSet<String> = rules.iter().map(|rule| rule.id.clone()).collect();
        let order: Vec<&str> = plan_order(&rules, &known)
            .unwrap()
            .into_iter()
            .map(|index| rules[index].id.as_str())
            .collect();
        let position = |id: &str| order.iter().position(|other| *other == id).unwrap();
        assert!(position("core/gitignore-basics") < position("core/no-committed-env"));
    }
}
