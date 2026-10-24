// Loads egit's WebAssembly; it starts itself in the page's canvas.
import init from "./egit_web.js";

init().catch((error) => {
  const problem = document.getElementById("problem");
  problem.textContent = `egit couldn't load: ${error}`;
  problem.hidden = false;
});
