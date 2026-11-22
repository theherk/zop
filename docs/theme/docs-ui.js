// Put the zop logo in the mdBook menu bar title, next to the text, and keep it in step
// with the theme. `path_to_root` is set by mdBook on every page, so the path works at
// any depth.
(function () {
  var menuTitle = document.querySelector(".menu-bar .menu-title");
  if (!menuTitle) return;

  var root = typeof path_to_root === "string" ? path_to_root : "";
  var html = document.documentElement;
  var isDark = function () {
    return ["coal", "navy", "ayu"].some(function (name) {
      return html.classList.contains(name);
    });
  };

  var img = document.createElement("img");
  img.alt = "zop";
  var update = function () {
    img.src = root + "assets/" + (isDark() ? "zop-dark.svg" : "zop-light.svg");
  };
  update();

  var text = document.createElement("span");
  text.textContent = "zop";

  menuTitle.textContent = "";
  menuTitle.appendChild(img);
  menuTitle.appendChild(text);

  new MutationObserver(update).observe(html, {
    attributes: true,
    attributeFilter: ["class"],
  });
})();
