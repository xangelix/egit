// One of egit's workers: the page's WebAssembly again, on the page's own memory, taking the
// work egit hands its workers. The page sends its module and memory, once.
import init, { egit_worker } from "./egit_web.js";

self.onmessage = async ({ data: [module, memory] }) => {
  self.onmessage = null;
  await init({ module_or_path: module, memory });
  egit_worker();
};
