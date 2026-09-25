/** 内联 SVG 图标表(006 契约 §2.2):自绘 24×24 线性路径,零外部依赖(宪章 III),
 *  不复制第三方图标资产(宪章 V)。颜色经 currentColor 继承,悬停变色由按钮样式控制。 */
export type IconName =
  | "search"
  | "qr"
  | "refresh"
  | "shield"
  | "edit"
  | "power"
  | "trash"
  | "list"
  | "grid";

const PATHS: Record<IconName, string> = {
  search: '<circle cx="11" cy="11" r="7"/><path d="m16.6 16.6 4.4 4.4"/>',
  qr:
    '<rect x="3.5" y="3.5" width="7" height="7" rx="1.2"/>' +
    '<rect x="13.5" y="3.5" width="7" height="7" rx="1.2"/>' +
    '<rect x="3.5" y="13.5" width="7" height="7" rx="1.2"/>' +
    '<path d="M13.6 13.6h.01M20.4 13.6h.01M13.6 20.4h.01M20.4 20.4h.01M17 17h.01"/>',
  refresh:
    '<path d="M21 12a9 9 0 1 1-2.64-6.36"/><path d="M21 3.5V9h-5.5"/>',
  shield:
    '<path d="M12 3 19 5.6v5.1c0 4.7-2.9 7.9-7 9.8-4.1-1.9-7-5.1-7-9.8V5.6L12 3z"/>' +
    '<path d="m9 11.5 2.2 2.2 4.3-4.7"/>',
  edit: '<path d="M14.5 5.5 18.5 9.5 8 20H4v-4L14.5 5.5z"/><path d="m12.5 7.5 4 4"/>',
  power: '<path d="M16.95 6.05a7 7 0 1 1-9.9 0"/><path d="M12 2.5V12"/>',
  trash:
    '<path d="M4 7h16"/>' +
    '<path d="M9.5 7V5.5A1.5 1.5 0 0 1 11 4h2a1.5 1.5 0 0 1 1.5 1.5V7"/>' +
    '<path d="M6.5 7l.75 11.2A2 2 0 0 0 9.24 20h5.52a2 2 0 0 0 2-1.8L17.5 7"/>' +
    '<path d="M10 11v5M14 11v5"/>',
  list: '<path d="M8.5 6h12M8.5 12h12M8.5 18h12"/><path d="M4 6h.01M4 12h.01M4 18h.01"/>',
  grid:
    '<rect x="3.5" y="3.5" width="7.5" height="7.5" rx="1.5"/>' +
    '<rect x="13" y="3.5" width="7.5" height="7.5" rx="1.5"/>' +
    '<rect x="3.5" y="13" width="7.5" height="7.5" rx="1.5"/>' +
    '<rect x="13" y="13" width="7.5" height="7.5" rx="1.5"/>'
};

export const ICON_NAMES: readonly IconName[] = Object.keys(PATHS) as IconName[];

/** 渲染图标;经 <template> 解析,避免 SVG 片段的命名空间解析差异(jsdom 兼容)。 */
export function icon(name: IconName, size = 18): SVGSVGElement {
  const markup = PATHS[name];
  if (markup === undefined) throw new Error(`未知图标:${String(name)}`);
  const tpl = document.createElement("template");
  tpl.innerHTML =
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="${size}" height="${size}" ` +
    'fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" ' +
    'aria-hidden="true" focusable="false">' +
    markup +
    "</svg>";
  const svg = tpl.content.firstElementChild;
  if (!(svg instanceof SVGSVGElement)) throw new Error(`图标渲染失败:${name}`);
  return svg;
}
