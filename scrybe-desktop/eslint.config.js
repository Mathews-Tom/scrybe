import js from "@eslint/js";
import { defineConfig, globalIgnores } from "eslint/config";
import jsxA11y from "eslint-plugin-jsx-a11y";
import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";
import tseslint from "typescript-eslint";

// `eslint-plugin-jsx-a11y` ships no type declarations, so its export is
// `any`. Narrowing it here keeps `no-unsafe-argument` meaningful for the
// rest of the file instead of disabling the rule wholesale.
/* eslint-disable @typescript-eslint/no-unsafe-assignment, @typescript-eslint/no-unsafe-member-access --
   the plugin ships no declarations, so its default export is `any`; the
   assertion below is the one place that fact is allowed to surface. */
/** @type {import("eslint").Linter.Config} */
const jsxA11yStrict = jsxA11y.flatConfigs.strict;
/* eslint-enable @typescript-eslint/no-unsafe-assignment, @typescript-eslint/no-unsafe-member-access */

export default defineConfig([
  globalIgnores(["dist", "src-tauri/target"]),
  js.configs.recommended,
  tseslint.configs.strictTypeChecked,
  tseslint.configs.stylisticTypeChecked,
  reactHooks.configs.flat.recommended,
  jsxA11yStrict,
  {
    languageOptions: {
      globals: { ...globals.browser },
      parserOptions: {
        projectService: {
          allowDefaultProject: ["eslint.config.js"],
        },
        tsconfigRootDir: import.meta.dirname,
      },
    },
  },
  {
    files: ["eslint.config.js", "vite.config.ts"],
    languageOptions: { globals: { ...globals.node } },
  },
]);
