/** Tailwind v3 config for the GritIRT console (built by Trunk's tailwind-css pipeline). */
module.exports = {
  content: ["./index.html", "./src/**/*.{rs,html,js}"],
  theme: {
    extend: {
      colors: {
        ink: "#0a0e14",
        panel: "#11161f",
        panel2: "#1a212d",
        edge: "#232c3b",
        acc: "#3ddc97",
        accdim: "#1f8f66",
      },
      fontFamily: {
        mono: ['ui-monospace', '"Cascadia Code"', '"SF Mono"', 'Menlo', 'Consolas', 'monospace'],
      },
    },
  },
  plugins: [],
};
