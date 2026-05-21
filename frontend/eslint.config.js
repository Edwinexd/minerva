import js from '@eslint/js'
import globals from 'globals'
import reactHooks from 'eslint-plugin-react-hooks'
import reactRefresh from 'eslint-plugin-react-refresh'
import jsxA11y from 'eslint-plugin-jsx-a11y'
import tseslint from 'typescript-eslint'
import { defineConfig, globalIgnores } from 'eslint/config'

export default defineConfig([
  globalIgnores(['dist']),
  {
    files: ['**/*.{ts,tsx}'],
    extends: [
      js.configs.recommended,
      tseslint.configs.recommended,
      reactHooks.configs.flat.recommended,
      reactRefresh.configs.vite,
      // Static accessibility linting. DSV-IT requires new sites to meet the
      // accessibility law (WCAG 2.2); the `strict` ruleset is the most
      // comprehensive jsx-a11y baseline. Enforced in pre-commit + CI.
      jsxA11y.flatConfigs.strict,
    ],
    languageOptions: {
      ecmaVersion: 2020,
      globals: globals.browser,
    },
    rules: {
      // Rules not enabled by the strict preset but relevant to WCAG 2.2.
      // 4.1.2 Name, Role, Value: an element hidden from the a11y tree must
      // not also be focusable (creates a phantom tab stop).
      'jsx-a11y/no-aria-hidden-on-focusable': 'error',
      // Prefer native semantic elements over ARIA roles where one exists
      // (1.3.1 Info and Relationships, 4.1.2).
      'jsx-a11y/prefer-tag-over-role': 'error',
    },
  },
  {
    // shadcn/ui generated files intentionally export both components and
    // variant helpers (e.g. buttonVariants) from the same file.
    files: ['src/components/ui/**/*.{ts,tsx}'],
    rules: {
      'react-refresh/only-export-components': 'off',
      // The Label primitive is a generic <label> wrapper; the control it
      // labels is supplied by callers via htmlFor/id, which the static rule
      // cannot see. The actual label/control association is enforced at
      // runtime by the axe component tests and the pa11y CI job.
      'jsx-a11y/label-has-associated-control': 'off',
    },
  },
])
