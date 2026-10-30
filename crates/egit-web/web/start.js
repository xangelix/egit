// Loads egit's WebAssembly; it starts itself in the page's canvas. index.html adds this script
// from its head, so it may run before the body is read: it waits for the page first.
import init from "./egit_web.js";

const parsed =
  document.readyState === "loading"
    ? new Promise((resolve) =>
        document.addEventListener("DOMContentLoaded", resolve, { once: true }),
      )
    : Promise.resolve();

// A phone's keyboard covers the bottom of the page rather than making it shorter (Safari's;
// Chrome's too, by default): the page is kept to what shows, so egit lays itself out above the
// keyboard and the field typed into stays in view. The page itself never scrolls.
const shown = window.visualViewport;
if (shown) {
  const fit = () => {
    document.documentElement.style.height = `${shown.height}px`;
    window.scrollTo(0, 0);
  };
  shown.addEventListener("resize", fit);
  fit();
}

parsed
  .then(() => init())
  .catch((error) => {
    const problem = document.getElementById("problem");
    problem.textContent = `egit couldn't load: ${error}`;
    problem.hidden = false;
  });
