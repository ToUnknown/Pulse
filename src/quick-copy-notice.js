const notice = document.querySelector(".notice");
if (window.pulseQuickCopySucceeded) {
  notice.dataset.kind = "success";
  notice.textContent = "Text copied";
}
