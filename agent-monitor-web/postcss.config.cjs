// 补回 webpack 时期 postcss-loader 里的 autoprefixer。
// Vite 自己不加厂商前缀（它只按 build.cssTarget 做语法降级），少了这一条，
// iOS/WKWebView 壳里那些还需要 -webkit- 前缀的属性会静默失效。
module.exports = {
  plugins: [require("autoprefixer")],
};
