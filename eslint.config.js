import js from '@eslint/js';
import prettier from 'eslint-config-prettier';
import { defineConfig } from 'eslint/config';
import tseslint from 'typescript-eslint';

export default defineConfig(
  {
    ignores: ['**/dist/**', '**/node_modules/**', 'target/**'],
  },
  js.configs.recommended,
  tseslint.configs.strict,
  {
    rules: {
      '@typescript-eslint/no-explicit-any': 'error',
      'no-restricted-syntax': [
        'error',
        {
          selector: 'ExportDefaultDeclaration',
          message: 'Use named exports (AGENTS.md code style).',
        },
      ],
    },
  },
  {
    // Tool config files and rule modules must default-export by contract.
    files: ['*.config.{js,ts}', '**/rules/*/rule.ts'],
    rules: { 'no-restricted-syntax': 'off' },
  },
  {
    // Plain Node scripts: benchmarks, packaging, and the npm launcher.
    files: ['bench/**/*.mjs', 'scripts/**/*.mjs', 'packages/cli/bin/*.js'],
    languageOptions: { globals: { console: 'readonly', process: 'readonly' } },
  },
  prettier,
);
