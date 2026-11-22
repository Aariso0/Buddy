import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";

const mount = document.getElementById("root");
if (!mount) {
  throw new Error("Buddy: missing #root mount point");
}

ReactDOM.createRoot(mount).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
