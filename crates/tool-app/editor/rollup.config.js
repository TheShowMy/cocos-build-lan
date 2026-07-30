import { nodeResolve } from "@rollup/plugin-node-resolve";

export default {
  input: "src/editor.js",
  output: {
    file: "../assets/editor.bundle.js",
    format: "iife",
    compact: true
  },
  plugins: [
    nodeResolve(),
    {
      name: "strip-trailing-whitespace",
      generateBundle(_options, bundle) {
        for (const item of Object.values(bundle)) {
          if (item.type === "chunk") {
            item.code = item.code.replace(/[ \t]+$/gm, "");
          }
        }
      }
    }
  ]
};
