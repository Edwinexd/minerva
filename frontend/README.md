# React + TypeScript + Vite

This template provides a minimal setup to get React working in Vite with HMR and some ESLint rules.

## Accessibility (WCAG 2.2)

DSV-IT requires new sites to meet the accessibility law; the target standard is
WCAG 2.2 (level AA). Compliance is enforced by three layers, all wired into
pre-commit and CI:

1. **Static lint** ; `eslint-plugin-jsx-a11y` (`strict` ruleset) runs as part of
   `npm run lint`. Catches markup-level issues (missing alt text, label/control
   associations, ARIA misuse, keyboard handlers) across every component.
2. **Rendered component checks** ; `npm run test:run` renders components into
   jsdom and runs `axe-core` (WCAG 2.2 AA tags) against the result. Tests live
   next to the harness in `src/test/` (`*.test.tsx`). Use the helpers in
   `src/test/a11y.tsx` (`renderWithProviders`, the configured `axe`) and assert
   `expect(await axe(container)).toHaveNoViolations()`.
3. **End-to-end checks** ; `npm run pa11y` builds nothing on its own; in CI the
   `frontend-a11y` job builds the app, serves it with `vite preview`, and runs
   `pa11y-ci` (htmlcs WCAG2AA + axe) against the public routes in
   `.pa11yci.json`. This is where color contrast and other render-time criteria
   are verified in a real browser (jsdom cannot compute layout/contrast).

Modal dialogs use the native `<dialog>` element opened with `showModal()` so
focus trapping, Escape-to-close, top-layer rendering and the `::backdrop` are
provided by the platform rather than hand-rolled.

Type-checking note: test files are excluded from the production `tsc -b`
(via `tsconfig.app.json`) and type-checked separately with
`npm run typecheck:test` (`tsconfig.test.json`), which adds the vitest/jsdom
globals without leaking them into the app build.

Currently, two official plugins are available:

- [@vitejs/plugin-react](https://github.com/vitejs/vite-plugin-react/blob/main/packages/plugin-react) uses [Oxc](https://oxc.rs)
- [@vitejs/plugin-react-swc](https://github.com/vitejs/vite-plugin-react/blob/main/packages/plugin-react-swc) uses [SWC](https://swc.rs/)

## React Compiler

The React Compiler is not enabled on this template because of its impact on dev & build performances. To add it, see [this documentation](https://react.dev/learn/react-compiler/installation).

## Expanding the ESLint configuration

If you are developing a production application, we recommend updating the configuration to enable type-aware lint rules:

```js
export default defineConfig([
  globalIgnores(['dist']),
  {
    files: ['**/*.{ts,tsx}'],
    extends: [
      // Other configs...

      // Remove tseslint.configs.recommended and replace with this
      tseslint.configs.recommendedTypeChecked,
      // Alternatively, use this for stricter rules
      tseslint.configs.strictTypeChecked,
      // Optionally, add this for stylistic rules
      tseslint.configs.stylisticTypeChecked,

      // Other configs...
    ],
    languageOptions: {
      parserOptions: {
        project: ['./tsconfig.node.json', './tsconfig.app.json'],
        tsconfigRootDir: import.meta.dirname,
      },
      // other options...
    },
  },
])
```

You can also install [eslint-plugin-react-x](https://github.com/Rel1cx/eslint-react/tree/main/packages/plugins/eslint-plugin-react-x) and [eslint-plugin-react-dom](https://github.com/Rel1cx/eslint-react/tree/main/packages/plugins/eslint-plugin-react-dom) for React-specific lint rules:

```js
// eslint.config.js
import reactX from 'eslint-plugin-react-x'
import reactDom from 'eslint-plugin-react-dom'

export default defineConfig([
  globalIgnores(['dist']),
  {
    files: ['**/*.{ts,tsx}'],
    extends: [
      // Other configs...
      // Enable lint rules for React
      reactX.configs['recommended-typescript'],
      // Enable lint rules for React DOM
      reactDom.configs.recommended,
    ],
    languageOptions: {
      parserOptions: {
        project: ['./tsconfig.node.json', './tsconfig.app.json'],
        tsconfigRootDir: import.meta.dirname,
      },
      // other options...
    },
  },
])
```
