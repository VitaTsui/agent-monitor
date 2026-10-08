import React, { useEffect, useRef, useState } from "react";

import { Icon } from "@hsu-react/ui";

import styles from "./index.module.scss";

const NATURAL_WIDTH = 960;
const DEFAULT_SLIDE_CX = 12_192_000;
const DEFAULT_SLIDE_CY = 6_858_000;
const REL_NS =
  "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
  rotate?: number;
}

interface TextStyle {
  color: string;
  fontSize: number;
  fontFamily?: string;
  fontWeight?: number;
  fontStyle?: "italic";
  textAlign?: React.CSSProperties["textAlign"];
  justifyContent?: React.CSSProperties["justifyContent"];
  padding: string;
}

interface ShapeItem extends Rect {
  kind: "shape";
  key: string;
  fill: string;
  borderColor: string;
  borderWidth: number;
  radius: string;
  text: string;
  textStyle: TextStyle;
}

interface ImageItem extends Rect {
  kind: "image";
  key: string;
  src: string;
}

interface TableItem extends Rect {
  kind: "table";
  key: string;
  rows: string[][];
}

type SlideItem = ShapeItem | ImageItem | TableItem;

interface SlideModel {
  key: string;
  height: number;
  background: string;
  items: SlideItem[];
}

interface Relationship {
  target: string;
  type: string;
  external: boolean;
}

type RelationshipMap = Map<string, Relationship>;
type EntryMap = Map<string, Uint8Array>;

const children = (node: Element | Document, name: string): Element[] =>
  Array.from(node.children).filter((child) => child.localName === name);

const child = (
  node: Element | Document | null | undefined,
  name: string,
): Element | undefined =>
  node
    ? Array.from(node.children).find((item) => item.localName === name)
    : undefined;

const first = (
  node: Element | Document | null | undefined,
  name: string,
): Element | undefined =>
  node?.getElementsByTagNameNS("*", name)?.[0] as Element | undefined;

const numberAttr = (node: Element | undefined, name: string): number => {
  const value = Number(node?.getAttribute(name));
  return Number.isFinite(value) ? value : 0;
};

const normalizePath = (path: string): string => {
  const out: string[] = [];
  path
    .replace(/^\//, "")
    .split("/")
    .forEach((part) => {
      if (!part || part === ".") {
        return;
      }
      if (part === "..") {
        out.pop();
      } else {
        out.push(part);
      }
    });
  return out.join("/");
};

const resolvePart = (part: string, target: string): string => {
  if (target.startsWith("/")) {
    return normalizePath(target);
  }
  const slash = part.lastIndexOf("/");
  const dir = slash >= 0 ? part.slice(0, slash + 1) : "";
  return normalizePath(`${dir}${target}`);
};

const relsPath = (part: string): string => {
  const slash = part.lastIndexOf("/");
  const dir = slash >= 0 ? part.slice(0, slash + 1) : "";
  const name = slash >= 0 ? part.slice(slash + 1) : part;
  return `${dir}_rels/${name}.rels`;
};

const parseXml = (entries: EntryMap, path: string): Document | null => {
  const bytes = entries.get(path);
  if (!bytes) {
    return null;
  }
  const xml = new TextDecoder("utf-8").decode(bytes);
  const doc = new DOMParser().parseFromString(xml, "application/xml");
  return doc.getElementsByTagName("parsererror").length ? null : doc;
};

const parseRelationships = (
  entries: EntryMap,
  part: string,
): RelationshipMap => {
  const doc = parseXml(entries, relsPath(part));
  const out: RelationshipMap = new Map();
  if (!doc) {
    return out;
  }
  Array.from(doc.getElementsByTagNameNS("*", "Relationship")).forEach((rel) => {
    const id = rel.getAttribute("Id");
    const target = rel.getAttribute("Target");
    if (!id || !target) {
      return;
    }
    out.set(id, {
      target: resolvePart(part, target),
      type: rel.getAttribute("Type") ?? "",
      external: rel.getAttribute("TargetMode") === "External",
    });
  });
  return out;
};

const colorFromNode = (
  node: Element | undefined,
  theme: Map<string, string>,
): string | undefined => {
  if (!node) {
    return undefined;
  }
  const srgb = first(node, "srgbClr")?.getAttribute("val");
  if (srgb) {
    return `#${srgb}`;
  }
  const system = first(node, "sysClr");
  if (system) {
    const value = system.getAttribute("lastClr") ?? system.getAttribute("val");
    return value ? `#${value}` : undefined;
  }
  const scheme = first(node, "schemeClr")?.getAttribute("val");
  if (!scheme) {
    return undefined;
  }
  const alias: Record<string, string> = {
    bg1: "lt1",
    bg2: "lt2",
    tx1: "dk1",
    tx2: "dk2",
  };
  return theme.get(alias[scheme] ?? scheme);
};

const fillFrom = (
  node: Element | undefined,
  theme: Map<string, string>,
): string | undefined => {
  if (!node || first(node, "noFill")) {
    return undefined;
  }
  const solid = first(node, "solidFill");
  if (solid) {
    return colorFromNode(solid, theme);
  }
  const gradient = first(node, "gradFill");
  if (gradient) {
    return colorFromNode(first(gradient, "gs"), theme);
  }
  return undefined;
};

const parseTheme = (entries: EntryMap): Map<string, string> => {
  const themePath = Array.from(entries.keys()).find((path) =>
    /^ppt\/theme\/theme\d+\.xml$/i.test(path),
  );
  const doc = themePath ? parseXml(entries, themePath) : null;
  const out = new Map<string, string>();
  const scheme = first(doc ?? undefined, "clrScheme");
  if (!scheme) {
    return out;
  }
  Array.from(scheme.children).forEach((node) => {
    const color = colorFromNode(node, new Map());
    if (color) {
      out.set(node.localName, color);
    }
  });
  return out;
};

const rawRect = (node: Element | undefined): Rect | null => {
  const xfrm = first(node, "xfrm");
  if (!xfrm) {
    return null;
  }
  const off = first(xfrm, "off");
  const ext = first(xfrm, "ext");
  const width = numberAttr(ext, "cx");
  const height = numberAttr(ext, "cy");
  if (!width || !height) {
    return null;
  }
  return {
    x: numberAttr(off, "x"),
    y: numberAttr(off, "y"),
    width,
    height,
    rotate: numberAttr(xfrm, "rot") / 60_000 || undefined,
  };
};

const placeholderKey = (node: Element): string | null => {
  const ph = first(node, "ph");
  if (!ph) {
    return null;
  }
  const index = ph.getAttribute("idx");
  return index ? `idx:${index}` : `type:${ph.getAttribute("type") ?? "body"}`;
};

const placeholderRects = (doc: Document | null): Map<string, Rect> => {
  const out = new Map<string, Rect>();
  if (!doc) {
    return out;
  }
  Array.from(doc.getElementsByTagNameNS("*", "sp")).forEach((shape) => {
    const key = placeholderKey(shape);
    const rect = rawRect(child(shape, "spPr"));
    if (key && rect) {
      out.set(key, rect);
      const type = first(shape, "ph")?.getAttribute("type");
      if (type) {
        out.set(`type:${type}`, rect);
      }
    }
  });
  return out;
};

const mergeRects = (...maps: Map<string, Rect>[]): Map<string, Rect> => {
  const out = new Map<string, Rect>();
  maps.forEach((map) => map.forEach((value, key) => out.set(key, value)));
  return out;
};

const toPixels = (rect: Rect, slideCx: number): Rect => {
  const scale = NATURAL_WIDTH / slideCx;
  return {
    x: rect.x * scale,
    y: rect.y * scale,
    width: rect.width * scale,
    height: rect.height * scale,
    rotate: rect.rotate,
  };
};

const shapeText = (shape: Element): string => {
  const body = child(shape, "txBody");
  if (!body) {
    return "";
  }
  return children(body, "p")
    .map((paragraph) => {
      const prefix =
        first(paragraph, "buChar")?.getAttribute("char") ??
        (first(paragraph, "buAutoNum") ? "1." : "");
      const text = Array.from(paragraph.getElementsByTagNameNS("*", "t"))
        .map((node) => node.textContent ?? "")
        .join("");
      return prefix ? `${prefix} ${text}` : text;
    })
    .join("\n");
};

const readTextStyle = (
  shape: Element,
  theme: Map<string, string>,
): TextStyle => {
  const paragraph = first(shape, "p");
  const runStyle = first(paragraph, "rPr") ?? first(paragraph, "defRPr");
  const placeholderType = first(shape, "ph")?.getAttribute("type") ?? "";
  const size = numberAttr(runStyle, "sz") / 100;
  const pointSize = size || (placeholderType.includes("title") ? 28 : 18);
  const body = child(shape, "txBody");
  const bodyPr = child(body, "bodyPr");
  const anchor = bodyPr?.getAttribute("anchor");
  const align = first(paragraph, "pPr")?.getAttribute("algn");
  const family =
    first(runStyle, "latin")?.getAttribute("typeface") ?? undefined;
  const inset = (name: string, fallback: number): number =>
    (numberAttr(bodyPr, name) || fallback) / 9_525;

  return {
    color: colorFromNode(runStyle, theme) ?? theme.get("dk1") ?? "#1f2937",
    fontSize: pointSize * (96 / 72),
    fontFamily: family,
    fontWeight:
      runStyle?.getAttribute("b") === "1" || placeholderType.includes("title")
        ? 700
        : undefined,
    fontStyle: runStyle?.getAttribute("i") === "1" ? "italic" : undefined,
    textAlign: align === "ctr" ? "center" : align === "r" ? "right" : "left",
    justifyContent:
      anchor === "ctr" ? "center" : anchor === "b" ? "flex-end" : "flex-start",
    padding: `${inset("tIns", 45_720)}px ${inset("rIns", 91_440)}px ${inset(
      "bIns",
      45_720,
    )}px ${inset("lIns", 91_440)}px`,
  };
};

const mimeFor = (path: string): string => {
  const ext = path.slice(path.lastIndexOf(".") + 1).toLowerCase();
  const map: Record<string, string> = {
    jpg: "image/jpeg",
    jpeg: "image/jpeg",
    png: "image/png",
    gif: "image/gif",
    svg: "image/svg+xml",
    webp: "image/webp",
    bmp: "image/bmp",
  };
  return map[ext] ?? "application/octet-stream";
};

interface ParseContext {
  entries: EntryMap;
  part: string;
  rels: RelationshipMap;
  theme: Map<string, string>;
  slideCx: number;
  placeholders: Map<string, Rect>;
  includePlaceholders: boolean;
  createUrl: (bytes: Uint8Array, mime: string) => string;
}

const parseShape = (
  shape: Element,
  index: number,
  context: ParseContext,
): ShapeItem | null => {
  const placeholder = placeholderKey(shape);
  if (placeholder && !context.includePlaceholders) {
    return null;
  }
  const raw =
    rawRect(child(shape, "spPr")) ??
    (placeholder ? (context.placeholders.get(placeholder) ?? null) : null);
  if (!raw) {
    return null;
  }
  const rect = toPixels(raw, context.slideCx);
  const props = child(shape, "spPr");
  const preset = first(props, "prstGeom")?.getAttribute("prst") ?? "";
  const line = first(props, "ln");
  const lineWidth = numberAttr(line, "w") / 9_525;
  return {
    kind: "shape",
    key: `${context.part}:shape:${index}`,
    ...rect,
    fill: fillFrom(props, context.theme) ?? "transparent",
    borderColor: fillFrom(line, context.theme) ?? "transparent",
    borderWidth: lineWidth || 0,
    radius:
      preset === "ellipse" ? "50%" : preset === "roundRect" ? "10px" : "0",
    text: shapeText(shape),
    textStyle: readTextStyle(shape, context.theme),
  };
};

const parseImage = (
  picture: Element,
  index: number,
  context: ParseContext,
): ImageItem | null => {
  const rect = rawRect(child(picture, "spPr"));
  const blip = first(picture, "blip");
  const relId =
    blip?.getAttributeNS(REL_NS, "embed") ?? blip?.getAttribute("r:embed");
  const rel = relId ? context.rels.get(relId) : undefined;
  const bytes =
    rel && !rel.external ? context.entries.get(rel.target) : undefined;
  if (!rect || !rel || !bytes) {
    return null;
  }
  return {
    kind: "image",
    key: `${context.part}:image:${index}`,
    ...toPixels(rect, context.slideCx),
    src: context.createUrl(bytes, mimeFor(rel.target)),
  };
};

const parseTable = (
  frame: Element,
  index: number,
  context: ParseContext,
): TableItem | null => {
  const rect = rawRect(frame);
  if (!rect) {
    return null;
  }
  const table = first(frame, "tbl");
  const rows = table
    ? Array.from(table.getElementsByTagNameNS("*", "tr")).map((row) =>
        Array.from(row.getElementsByTagNameNS("*", "tc")).map((cell) =>
          Array.from(cell.getElementsByTagNameNS("*", "t"))
            .map((text) => text.textContent ?? "")
            .join(""),
        ),
      )
    : [["图表"]];
  return {
    kind: "table",
    key: `${context.part}:table:${index}`,
    ...toPixels(rect, context.slideCx),
    rows,
  };
};

const parsePartItems = (
  doc: Document | null,
  context: ParseContext,
): SlideItem[] => {
  const tree = first(doc ?? undefined, "spTree");
  if (!tree) {
    return [];
  }
  const items: SlideItem[] = [];
  Array.from(tree.children).forEach((node, index) => {
    if (node.localName === "sp" || node.localName === "cxnSp") {
      const item = parseShape(node, index, context);
      if (item) {
        items.push(item);
      }
      return;
    }
    if (node.localName === "pic") {
      const item = parseImage(node, index, context);
      if (item) {
        items.push(item);
      }
      return;
    }
    if (node.localName === "graphicFrame") {
      const item = parseTable(node, index, context);
      if (item) {
        items.push(item);
      }
    }
  });
  return items;
};

const backgroundFrom = (
  doc: Document | null,
  theme: Map<string, string>,
): string | undefined => fillFrom(first(doc ?? undefined, "bg"), theme);

const entriesFromCfb = (cfb: {
  FullPaths: string[];
  FileIndex: Array<{ content?: Uint8Array }>;
}): EntryMap => {
  const out: EntryMap = new Map();
  cfb.FullPaths.forEach((fullPath, index) => {
    const content = cfb.FileIndex[index]?.content;
    const path = fullPath.replace(/^Root Entry\//, "").replace(/\/$/, "");
    if (path && content?.length) {
      out.set(path, new Uint8Array(content));
    }
  });
  return out;
};

const parsePresentation = async (
  bytes: Uint8Array,
  createUrl: (bytes: Uint8Array, mime: string) => string,
): Promise<SlideModel[]> => {
  // SheetJS 已经是表格预览的现有依赖；它内置的 CFB/ZIP 读取器也能读取 OOXML 的
  // PPTX 包。动态加载继续保证普通文件预览不承担这几百 KB。
  const XLSX = await import("xlsx");
  const archive = XLSX.CFB.read(bytes, { type: "buffer" }) as {
    FullPaths: string[];
    FileIndex: Array<{ content?: Uint8Array }>;
  };
  const entries = entriesFromCfb(archive);
  const presentationPath = "ppt/presentation.xml";
  const presentation = parseXml(entries, presentationPath);
  if (!presentation) {
    throw new Error("不是有效的 PPTX 文件");
  }

  const size = first(presentation, "sldSz");
  const slideCx = numberAttr(size, "cx") || DEFAULT_SLIDE_CX;
  const slideCy = numberAttr(size, "cy") || DEFAULT_SLIDE_CY;
  const height = NATURAL_WIDTH * (slideCy / slideCx);
  const theme = parseTheme(entries);
  const presentationRels = parseRelationships(entries, presentationPath);
  const slideParts = Array.from(
    presentation.getElementsByTagNameNS("*", "sldId"),
  )
    .map((slide) => {
      const relId =
        slide.getAttributeNS(REL_NS, "id") ?? slide.getAttribute("r:id");
      return relId ? presentationRels.get(relId)?.target : undefined;
    })
    .filter((path): path is string => !!path);

  if (!slideParts.length) {
    throw new Error("演示文稿里没有幻灯片");
  }

  return slideParts.map((slidePart, slideIndex) => {
    const slideDoc = parseXml(entries, slidePart);
    const slideRels = parseRelationships(entries, slidePart);
    const layoutRel = Array.from(slideRels.values()).find((rel) =>
      rel.type.endsWith("/slideLayout"),
    );
    const layoutPart = layoutRel?.target;
    const layoutDoc = layoutPart ? parseXml(entries, layoutPart) : null;
    const layoutRels = layoutPart
      ? parseRelationships(entries, layoutPart)
      : new Map<string, Relationship>();
    const masterRel = Array.from(layoutRels.values()).find((rel) =>
      rel.type.endsWith("/slideMaster"),
    );
    const masterPart = masterRel?.target;
    const masterDoc = masterPart ? parseXml(entries, masterPart) : null;
    const masterRels = masterPart
      ? parseRelationships(entries, masterPart)
      : new Map<string, Relationship>();
    const masterPlaceholders = placeholderRects(masterDoc);
    const layoutPlaceholders = mergeRects(
      masterPlaceholders,
      placeholderRects(layoutDoc),
    );
    const slidePlaceholders = mergeRects(
      masterPlaceholders,
      layoutPlaceholders,
    );

    const makeContext = (
      part: string,
      rels: RelationshipMap,
      placeholders: Map<string, Rect>,
      includePlaceholders: boolean,
    ): ParseContext => ({
      entries,
      part,
      rels,
      theme,
      slideCx,
      placeholders,
      includePlaceholders,
      createUrl,
    });

    const items = [
      ...(masterPart
        ? parsePartItems(
            masterDoc,
            makeContext(masterPart, masterRels, masterPlaceholders, false),
          )
        : []),
      ...(layoutPart
        ? parsePartItems(
            layoutDoc,
            makeContext(layoutPart, layoutRels, layoutPlaceholders, false),
          )
        : []),
      ...parsePartItems(
        slideDoc,
        makeContext(slidePart, slideRels, slidePlaceholders, true),
      ),
    ];

    return {
      key: `${slidePart}:${slideIndex}`,
      height,
      background:
        backgroundFrom(slideDoc, theme) ??
        backgroundFrom(layoutDoc, theme) ??
        backgroundFrom(masterDoc, theme) ??
        theme.get("lt1") ??
        "#ffffff",
      items,
    };
  });
};

const itemStyle = (item: Rect): React.CSSProperties => ({
  left: item.x,
  top: item.y,
  width: item.width,
  height: item.height,
  transform: item.rotate ? `rotate(${item.rotate}deg)` : undefined,
});

interface PptxViewProps {
  bytes: Uint8Array<ArrayBuffer>;
  fileName: string;
}

const PptxView: React.FC<PptxViewProps> = ({ bytes, fileName }) => {
  const rootRef = useRef<HTMLDivElement>(null);
  const [slides, setSlides] = useState<SlideModel[]>([]);
  const [scale, setScale] = useState(0.5);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    const root = rootRef.current;
    if (!root) {
      return undefined;
    }
    const resize = () =>
      setScale(
        Math.min(1, Math.max(0.2, (root.clientWidth - 24) / NATURAL_WIDTH)),
      );
    resize();
    const observer = new ResizeObserver(resize);
    observer.observe(root);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    let active = true;
    const urls: string[] = [];
    const createUrl = (content: Uint8Array, mime: string): string => {
      const copy = new Uint8Array(content.byteLength);
      copy.set(content);
      const url = URL.createObjectURL(new Blob([copy.buffer], { type: mime }));
      urls.push(url);
      return url;
    };

    setSlides([]);
    setError("");
    setLoading(true);
    parsePresentation(bytes, createUrl)
      .then((next) => {
        if (active) {
          setSlides(next);
          setLoading(false);
        } else {
          urls.forEach((url) => URL.revokeObjectURL(url));
        }
      })
      .catch((reason: unknown) => {
        urls.forEach((url) => URL.revokeObjectURL(url));
        if (active) {
          setError(reason instanceof Error ? reason.message : "PPTX 解析失败");
          setLoading(false);
        }
      });

    return () => {
      active = false;
      urls.forEach((url) => URL.revokeObjectURL(url));
    };
  }, [bytes]);

  return (
    <div ref={rootRef} className={styles.pptxPreview} aria-label={fileName}>
      {loading ? (
        <div className={styles.pptxStatus}>
          <Icon icon="ph:spinner-gap" className={styles.pptxSpinner} />
          <span>正在解析演示文稿…</span>
        </div>
      ) : null}
      {error ? (
        <div className={styles.pptxStatus}>
          <Icon icon="ph:warning-circle" />
          <span>{error}</span>
        </div>
      ) : null}
      {slides.map((slide, slideIndex) => (
        <section key={slide.key} className={styles.pptxPage}>
          <div className={styles.pptxPageLabel}>
            {slideIndex + 1} / {slides.length}
          </div>
          <div
            className={styles.pptxSlideFrame}
            style={{
              width: NATURAL_WIDTH * scale,
              height: slide.height * scale,
            }}
          >
            <div
              className={styles.pptxSlide}
              style={{
                width: NATURAL_WIDTH,
                height: slide.height,
                background: slide.background,
                transform: `scale(${scale})`,
              }}
            >
              {slide.items.map((item) => {
                if (item.kind === "image") {
                  return (
                    <img
                      key={item.key}
                      className={styles.pptxImage}
                      src={item.src}
                      alt=""
                      style={itemStyle(item)}
                    />
                  );
                }
                if (item.kind === "table") {
                  return (
                    <div
                      key={item.key}
                      className={styles.pptxTableWrap}
                      style={itemStyle(item)}
                    >
                      <table className={styles.pptxTable}>
                        <tbody>
                          {item.rows.map((row, rowIndex) => (
                            <tr key={`${item.key}:r:${rowIndex}`}>
                              {row.map((cellText, cellIndex) => (
                                <td
                                  key={`${item.key}:c:${rowIndex}:${cellIndex}`}
                                >
                                  {cellText}
                                </td>
                              ))}
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    </div>
                  );
                }
                return (
                  <div
                    key={item.key}
                    className={styles.pptxShape}
                    style={{
                      ...itemStyle(item),
                      background: item.fill,
                      borderColor: item.borderColor,
                      borderWidth: item.borderWidth,
                      borderRadius: item.radius,
                      color: item.textStyle.color,
                      fontSize: item.textStyle.fontSize,
                      fontFamily: item.textStyle.fontFamily,
                      fontWeight: item.textStyle.fontWeight,
                      fontStyle: item.textStyle.fontStyle,
                      textAlign: item.textStyle.textAlign,
                      justifyContent: item.textStyle.justifyContent,
                      padding: item.textStyle.padding,
                    }}
                  >
                    {item.text}
                  </div>
                );
              })}
            </div>
          </div>
        </section>
      ))}
    </div>
  );
};

export default PptxView;
