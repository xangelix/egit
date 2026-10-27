// Loads egit's WebAssembly; it starts itself in the page's canvas. index.html adds this script
// from its head, so it may run before the body is read: it waits for the page first.
import init from "./egit_web.js";

const parsed =
  document.readyState === "loading"
    ? new Promise((resolve) =>
        document.addEventListener("DOMContentLoaded", resolve, { once: true }),
      )
    : Promise.resolve();

parsed
  .then(() => init())
  .catch((error) => {
    const problem = document.getElementById("problem");
    problem.textContent = `egit couldn't load: ${error}`;
    problem.hidden = false;
  });
