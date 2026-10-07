import React from "react";
import ReactDOM from "react-dom/client";
import { HashRouter } from "react-router-dom";
import App from "./App";
import "./site.css";

// HashRouter：官网同时部署于 GitHub Pages 子路径与 Netlify，
// hash 路由无需任何服务端回退配置。
ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <HashRouter>
      <App />
    </HashRouter>
  </React.StrictMode>,
);
