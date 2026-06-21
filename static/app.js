// Small enhancements; everything works without JS except htmx flows.

function autosize(el) {
  el.style.height = "auto";
  el.style.height = el.scrollHeight + "px";
}

function localizeTimes(root) {
  root.querySelectorAll("time[datetime]").forEach(function (t) {
    var d = new Date(t.getAttribute("datetime"));
    if (!isNaN(d)) {
      t.textContent = d.toLocaleString(undefined, {
        year: "numeric", month: "short", day: "numeric",
        hour: "2-digit", minute: "2-digit",
      });
    }
  });
}

document.addEventListener("input", function (e) {
  if (e.target.matches("textarea")) autosize(e.target);
});

// Ctrl+Enter / Cmd+Enter submits the enclosing composer or editor form.
document.addEventListener("keydown", function (e) {
  if ((e.ctrlKey || e.metaKey) && e.key === "Enter" && e.target.matches("textarea")) {
    var form = e.target.closest("form");
    if (form) {
      e.preventDefault();
      if (window.htmx && form.hasAttribute("hx-post")) htmx.trigger(form, "submit");
      else if (window.htmx && form.hasAttribute("hx-put")) htmx.trigger(form, "submit");
      else form.requestSubmit();
    }
  }
});

// Copy share link buttons: <button data-copy="...">
document.addEventListener("click", function (e) {
  var btn = e.target.closest("[data-copy]");
  if (btn) {
    navigator.clipboard.writeText(new URL(btn.getAttribute("data-copy"), location.href).href);
    var old = btn.textContent;
    btn.textContent = "Copied!";
    setTimeout(function () { btn.textContent = old; }, 1200);
  }
});

document.addEventListener("DOMContentLoaded", function () {
  localizeTimes(document);
});

// Keep the hidden `resources` field in sync with the upload chips.
function syncResources(chipsEl) {
  var form = chipsEl.closest("form");
  if (!form) return;
  var hidden = form.querySelector(".resource-uids");
  if (hidden) {
    var uids = [];
    chipsEl.querySelectorAll(".chip[data-uid]").forEach(function (c) {
      uids.push(c.getAttribute("data-uid"));
    });
    hidden.value = uids.join(" ");
  }
  var fileInput = form.querySelector(".file-input");
  if (fileInput) fileInput.value = "";
}

document.addEventListener("click", function (e) {
  var rm = e.target.closest(".chip-remove");
  if (rm) {
    var chips = rm.closest(".attach-chips");
    rm.closest(".chip").remove();
    if (chips) syncResources(chips);
  }
});

// Re-localize timestamps and sync upload chips inside htmx-swapped fragments.
document.body.addEventListener("htmx:afterSwap", function (e) {
  localizeTimes(e.target);
  if (e.target.classList && e.target.classList.contains("attach-chips")) {
    syncResources(e.target);
  }
});
