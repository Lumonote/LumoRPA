// Runs inside the visible platform tab. The host supplies only an operation and data.
function acquisitionPage(op, args) {
  const sites = {
    xiaohongshu: { domains: ["xiaohongshu.com"], person: /^\/user\/profile\/([^/]+)\/?$/, post: /^\/(?:explore|discovery\/item|search_result)\/[^/]+/, cards: ".note-item,.note-detail-mask,.note-container,.comment-item" },
    douyin: { domains: ["douyin.com"], person: /^\/user\/([^/]+)\/?$/, post: /^\/(?:video|note)\/[^/]+/, cards: "[data-e2e*='search-card'],[data-e2e='search_result'],[data-e2e='comment-item'],[data-e2e='video-info']" },
    weibo: { domains: ["weibo.com","weibo.cn"], person: /^\/(?:u\/|n\/)?([0-9]+)\/?$/, post: /^\/(?:detail\/\w+|[0-9]+\/[a-zA-Z0-9]+)\/?$/, cards: ".card-wrap,.WB_cardwrap,[action-type='feed_list_item'],[class*='Feed_wrap'],[class*='CommentItem']" },
  };
  const site = sites[args.platform];
  const visible = el => !!el && el.getClientRects().length > 0 && getComputedStyle(el).visibility !== "hidden";
  const all = (node, sel) => Array.from(node.querySelectorAll(sel)).filter(visible);
  const clean = value => String(value || "").trim();
  const validUrl = value => {
    try {
      const u = new URL(value, location.href);
      return u.protocol === "https:" && !u.username && !u.password && !u.port &&
        site.domains.some(d => u.hostname === d || u.hostname.endsWith("." + d)) ? u : null;
    } catch { return null; }
  };
  const href = el => validUrl(el.href);
  const person = u => u && site.person.exec(u.pathname)?.[1];
  const post = u => u && site.post.test(u.pathname);
  const current = validUrl(location.href);
  if (!site || !current) throw Error("页面已离开所选平台，请检查登录或跳转。");
  const dialogs = all(document, "[role=dialog],[class*='login-modal'],[class*='captcha'],#captcha_container,.login-container");
  if (dialogs.some(el => /扫码登录|手机号登录|请先登录|安全验证|拖动滑块|验证码登录/.test(el.innerText)) ||
      /\/(?:passport|login|captcha)(?:\/|$)/.test(location.pathname) ||
      /访问受限|验证码|安全验证/.test(document.title)) throw Error("请在浏览器完成登录或验证后重新运行。");
  if (op === "discover") {
    const rows = [], links = new Set();
    for (const card of all(document, site.cards + ",article")) {
      const anchors = all(card, "a[href]");
      const posts = anchors.map(href).filter(post);
      posts.forEach(u => links.add(u.href));
      const authors = anchors.filter(a => person(href(a)));
      // Multiple authors in a nested card make attribution ambiguous.
      if (new Set(authors.map(a => person(href(a)))).size !== 1) continue;
      const author = authors.find(a => clean(a.innerText || a.title));
      if (!author) continue;
      const body = all(card, "[node-type=feed_list_content],.content,.note-text,.title,.desc,[data-e2e='video-desc'],[data-e2e='comment-level-1'],[class*='detailText']").find(el => clean(el.innerText));
      const source = posts[0] || (post(current) ? current : null);
      if (!body || !source) continue;
      rows.push({ platformUserId: person(href(author)), displayName: clean(author.innerText || author.title).slice(0,200),
        profileUrl: href(author).href, sourceUrl: source.href, evidence: clean(body.innerText).slice(0,2600) });
      if (rows.length >= 40) break;
    }
    for (const a of all(document,"a[href]")) if (post(href(a))) links.add(href(a).href);
    return { rows, links: [...links].slice(0,10), empty: /暂无(?:相关)?(?:结果|内容)|未找到相关|没有找到相关/.test(document.body.innerText) };
  }
  if (op === "guard") return document.hasFocus() && document.visibilityState === "visible";
  if (op === "isPost") return post(current);
  if (op === "gestureClear") {
    document.getElementById(args.token)?.remove();
    return true;
  }
  if (op === "gestureArm") {
    if (!document.hasFocus() || (args.stage === "chat" && person(current) !== args.userId)) return false;
    const glass = document.createElement("div");
    glass.id = args.token;
    glass.setAttribute("popover","manual");
    glass.style.cssText = "position:fixed!important;inset:0!important;width:100vw!important;height:100vh!important;margin:0!important;padding:0!important;border:0!important;background:transparent!important;pointer-events:auto!important;";
    document.documentElement.append(glass);
    glass.showPopover();
    const remember = (event) => {
      event.preventDefault(); event.stopImmediatePropagation();
      if (!event.isTrusted || Math.abs(event.screenX-args.x)>3 || Math.abs(event.screenY-args.y)>3) return;
      glass.dataset.point = JSON.stringify([event.clientX,event.clientY]);
    };
    for (const type of ["pointermove","pointerdown","pointerup","mousedown","mouseup","click"]) glass.addEventListener(type,remember);
    setTimeout(()=>glass.remove(),5000);
    return true;
  }
  if (op === "gesturePeek" || op === "gestureConsume") {
    const glass = document.getElementById(args.token);
    if (!document.hasFocus() || !glass?.matches(":popover-open") || !glass.dataset.point) return "";
    const [x,y] = JSON.parse(glass.dataset.point);
    const stack = document.elementsFromPoint(x,y);
    if (stack[0] !== glass) return "";
    const el = stack[1];
    const anchor = el?.closest("a[href]");
    let target, value;
    if (args.stage === "post" && post(anchor && href(anchor))) {
      target=anchor; value=href(anchor).href;
    } else if (args.stage === "chat" && person(current) === args.userId) {
      target=el?.closest("button,[role=button],a") || el;
      if (!target || !["私信","发私信","私聊","发消息"].includes(clean(target.getAttribute("aria-label") || target.innerText)) || target.matches("[disabled],[aria-disabled=true]")) return "";
      value="chat";
    } else return "";
    if (op === "gesturePeek") { glass._lumoTarget=target; return value; }
    if (target !== glass._lumoTarget || value!==args.expected) return "";
    glass.remove();
    if (value==="chat") target.click();
    return value;
  }
  if (op === "scroll") { window.scrollBy(0, Math.round(innerHeight * 0.8)); return true; }
  const id = args.userId;
  const oneButton = (root, labels) => {
    const hits = all(root, "button,[role=button],a,span,div").filter(el => labels.includes(clean(el.getAttribute("aria-label") || el.innerText)) && !el.matches("[disabled],[aria-disabled=true]"));
    const deepest = hits.filter(el => !hits.some(child => child !== el && el.contains(child)));
    return deepest.length === 1 ? deepest[0] : null;
  };
  if (op === "openChat") {
    if (person(current) !== id) throw Error("平台主页与所选客户不一致。");
    const contact = oneButton(document, ["私信","发私信","私聊","发消息"]);
    if (!contact) throw Error("没有找到唯一的私信入口，请确认账号可联系该客户。");
    contact.click(); return true;
  }
  const editorSelector = "textarea,[contenteditable=true],[role=textbox]";
  const roots = all(document, "[role=dialog],[class*='chat-box'],[class*='chatBox'],[class*='chat-container'],[class*='chatContainer'],[class*='im-chat'],[class*='ChatPanel'],[class*='chat-panel'],[class*='ChatBox'],[data-e2e='chat-room']");
  const peers = roots.filter(el => {
    const headers = all(el, "header,[class*='header'],[class*='Header'],[data-e2e='chat-header']");
    const ids = headers.flatMap(h => all(h, "a[href]").map(a => person(href(a))).filter(Boolean));
    const bound = el.getAttribute("data-peer-id") || el.getAttribute("data-recipient-id");
    return (!bound || bound === id) && ids.every(x => x === id) && (Boolean(bound) || ids.length > 0);
  });
  const boxes = peers.filter(el => !peers.some(child => child !== el && el.contains(child)));
  if (boxes.length !== 1) throw Error("未能从会话标题确认收件人主页或平台 ID。");
  const box = boxes[0];
  if (op === "peer") return true;
  if (op === "draftMatches") {
    const editors=all(box,editorSelector);
    return editors.length===1 && clean(editors[0].value ?? editors[0].innerText)===args.body;
  }
  function messages() {
    return all(box, "[data-message-id],[data-msg-id],[data-mid],.message-item,[class*='MessageItem'],[class*='messageItem']").flatMap(el => {
      const cls = String(el.className);
      const sender = el.getAttribute("data-sender-id");
      const direct = el.getAttribute("data-direction");
      const inbound = sender === id || direct === "inbound" || /\b(received|incoming|other-message)\b/.test(cls);
      const outbound = el.getAttribute("data-is-self") === "true" || direct === "outbound" || /\b(self|outgoing|from-me|sent-message)\b/.test(cls);
      if (inbound === outbound) return [];
      const body = all(el, "[data-message-text],.message-text,[class*='text-content'],[class*='messageText'],[class*='bubble']").at(-1);
      const text = clean(body?.innerText || el.innerText);
      if (!text || /发送失败|重新发送|发送中/.test(el.innerText) || el.querySelector("[data-status='failed'],[data-status='sending'],[class*='send-fail']")) return [];
      return [{ inbound, text, id: el.getAttribute("data-message-id") || el.getAttribute("data-msg-id") || el.getAttribute("data-mid") || "" }];
    });
  }
  if (op === "sync") {
    const found = messages();
    if (!found.length && clean(box.innerText) && !/暂无消息|还没有消息|开始聊天/.test(box.innerText)) throw Error("当前会话未识别到带方向的消息。");
    if (found.some(m => m.inbound && !m.id)) throw Error("平台没有提供可去重的入站消息标识，请手动记录回复。");
    return found.filter(m => m.inbound).slice(-100);
  }
  const outgoing = messages().filter(m => !m.inbound && m.text === args.body);
  if (op === "receipt") return outgoing.some(m => m.id && !args.before.includes(m.id));
  if (op === "prepare") {
    if (outgoing.length) throw Error("对话中已经存在相同的已发消息，请核对。");
    const editors = all(box, editorSelector);
    if (editors.length !== 1) throw Error("无法唯一确定私信输入框。");
    const input = editors[0];
    if (clean(input.value ?? input.innerText)) throw Error("输入框中已有草稿，请先处理。");
    input.focus();
    return outgoing.map(m => m.id);
  }
  if (op === "send") {
    const editors = all(box, editorSelector);
    if (editors.length !== 1 || clean(editors[0].value ?? editors[0].innerText) !== args.body || outgoing.length) return false;
    const send = oneButton(box, ["发送","发送消息","Send"]);
    if (!send) return false;
    send.click(); return true;
  }
  if (op === "fill") {
    const editors = all(box, editorSelector);
    const input = editors[0];
    if (editors.length !== 1 || input !== document.activeElement || clean(input.value ?? input.innerText)) return false;
    if (input.isContentEditable) return document.execCommand("insertText", false, args.body);
    const proto = input.tagName === "TEXTAREA" ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, "value").set.call(input, args.body);
    input.dispatchEvent(new Event("input", { bubbles: true }));
    return true;
  }
  throw Error("未知获客操作");
}
