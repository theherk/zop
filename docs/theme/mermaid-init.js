// Mermaid rendering for mdbook.
// Loads mermaid from CDN, finds ```mermaid code blocks, and renders them.
(function () {
  var MERMAID_CDN = "https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.min.js";

  var isDark =
    document.documentElement.classList.contains("coal") ||
    document.documentElement.classList.contains("navy") ||
    document.documentElement.classList.contains("ayu") ||
    document.documentElement.classList.contains("rust");

  var lightTheme = {
    primaryColor: "#eef2f7",
    primaryTextColor: "#1f2933",
    primaryBorderColor: "#3e5c76",
    secondaryColor: "#dde5ee",
    secondaryTextColor: "#1f2933",
    secondaryBorderColor: "#b6c4d4",
    tertiaryColor: "#f6f8fb",
    tertiaryTextColor: "#1f2933",
    tertiaryBorderColor: "#b6c4d4",
    lineColor: "#3e5c76",
    textColor: "#1f2933",
    mainBkg: "#eef2f7",
    nodeBorder: "#3e5c76",
    clusterBkg: "#f6f8fb",
    clusterBorder: "#b6c4d4",
    titleColor: "#1d3557",
    edgeLabelBackground: "#ffffff",
    nodeTextColor: "#1f2933",

    actorBkg: "#eef2f7",
    actorBorder: "#3e5c76",
    actorTextColor: "#1d3557",
    actorLineColor: "#3e5c76",
    signalColor: "#1f2933",
    signalTextColor: "#1f2933",
    labelBoxBkgColor: "#eef2f7",
    labelBoxBorderColor: "#3e5c76",
    labelTextColor: "#1d3557",
    loopTextColor: "#1d3557",
    noteBkgColor: "#fdf3d7",
    noteBorderColor: "#e0c36a",
    noteTextColor: "#1f2933",
    activationBkgColor: "#dde5ee",
    activationBorderColor: "#3e5c76",
    sequenceNumberColor: "#ffffff",
  };

  var darkTheme = {
    primaryColor: "#27384d",
    primaryTextColor: "#d5dde6",
    primaryBorderColor: "#7aa2c8",
    secondaryColor: "#1f2d3d",
    secondaryTextColor: "#d5dde6",
    secondaryBorderColor: "#35506b",
    tertiaryColor: "#223041",
    tertiaryTextColor: "#d5dde6",
    tertiaryBorderColor: "#35506b",
    lineColor: "#7aa2c8",
    textColor: "#d5dde6",
    mainBkg: "#27384d",
    nodeBorder: "#7aa2c8",
    clusterBkg: "#1f2d3d",
    clusterBorder: "#35506b",
    titleColor: "#a9c7e4",
    edgeLabelBackground: "#1b2633",
    nodeTextColor: "#d5dde6",

    actorBkg: "#27384d",
    actorBorder: "#7aa2c8",
    actorTextColor: "#d5dde6",
    actorLineColor: "#7aa2c8",
    signalColor: "#d5dde6",
    signalTextColor: "#d5dde6",
    labelBoxBkgColor: "#27384d",
    labelBoxBorderColor: "#7aa2c8",
    labelTextColor: "#a9c7e4",
    loopTextColor: "#a9c7e4",
    noteBkgColor: "#3d3520",
    noteBorderColor: "#8a7a3a",
    noteTextColor: "#e6dcc0",
    activationBkgColor: "#1f2d3d",
    activationBorderColor: "#7aa2c8",
    sequenceNumberColor: "#1b2633",
  };

  function renderDiagrams() {
    mermaid.initialize({
      startOnLoad: false,
      theme: "base",
      themeVariables: isDark ? darkTheme : lightTheme,
      securityLevel: "loose",
      flowchart: { useMaxWidth: true, htmlLabels: true, curve: "basis" },
      sequence: { useMaxWidth: true, mirrorActors: false },
    });

    var blocks = document.querySelectorAll("code.language-mermaid");
    blocks.forEach(function (codeEl) {
      var pre = codeEl.parentElement;
      var div = document.createElement("div");
      div.className = "mermaid";
      div.textContent = codeEl.textContent;
      pre.parentElement.replaceChild(div, pre);
    });

    mermaid.run({ nodes: document.querySelectorAll(".mermaid") });
  }

  if (document.querySelectorAll("code.language-mermaid").length === 0) return;

  var script = document.createElement("script");
  script.src = MERMAID_CDN;
  script.onload = renderDiagrams;
  document.head.appendChild(script);
})();
