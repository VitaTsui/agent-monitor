const path = require("path");
const uppercamelcase = require("uppercamelcase");
const {
  readAndReplaceTemplate,
  writeFile,
  validatePanelName,
  buildApiPath,
  buildPermiPrefix,
} = require("./utils.cjs");

/**
 * 创建 Modal 列表面板（带表单）
 * 用法: node createListModalPanel.cjs <panelName> [parent] [apiUrl] [parentIdProp] [modalTitle]
 * 示例: node createListModalPanel.cjs userAbnormalPolicy ApiExceptionStrategyManagement /vm/monitApiExcepStratUser excepStratId "用户异常策略管理"
 */
function main() {
  try {
    const panelName = process.argv[2];
    validatePanelName(panelName);

    const PanelName = uppercamelcase(panelName);
    const parent = process.argv[3] || "";
    const apiUrl = process.argv[4] || "";
    const parentIdProp = process.argv[5] || "parentId";
    const modalTitle = process.argv[6] || PanelName;

    const panelPath = path.resolve(
      __dirname,
      "../src/pages",
      parent,
      PanelName
    );
    const apiName = PanelName.toLowerCase();
    const apiPath = path.resolve(__dirname, "../src/services/apis", parent);
    const apiPathValue = buildApiPath(parent, apiName);
    const permiPrefix = buildPermiPrefix(apiUrl);
    const nameLower = PanelName.charAt(0).toLowerCase() + PanelName.slice(1);

    console.log(`\n开始创建 Modal 列表面板: ${PanelName}`);
    console.log(`面板路径: ${panelPath}`);
    console.log(`API 路径: ${apiPathValue}`);
    console.log(`权限前缀: ${permiPrefix || "无"}`);
    console.log(`父级ID属性: ${parentIdProp}`);
    console.log(`Modal 标题: ${modalTitle}\n`);

    // ========== 创建 Modal 列表部分 ==========
    const modalPanelTemplate = path.resolve(
      __dirname,
      "./ListModalPanel/panel.js"
    );
    const modalStoreTemplate = path.resolve(
      __dirname,
      "./ListModalPanel/store.js"
    );
    const modalApiTemplate = path.resolve(__dirname, "./ListModalPanel/api.js");

    const ModalPanel = readAndReplaceTemplate(modalPanelTemplate, {
      "<NAME>": PanelName,
      "<NAME_LOWER>": nameLower,
      "<PARENT_ID_PROP>": parentIdProp,
      "<MODAL_TITLE>": modalTitle,
      "<ADD_PERMI>": permiPrefix ? `"${permiPrefix}:add"` : "",
      "<EDIT_PERMI>": permiPrefix ? `"${permiPrefix}:upd"` : "",
      "<DELETE_PERMI>": permiPrefix ? `"${permiPrefix}:del"` : "",
      "<QUERY_PERMI>": permiPrefix ? `"${permiPrefix}:query"` : "",
    });

    const ModalStore = readAndReplaceTemplate(modalStoreTemplate, {
      "<NAME>": PanelName,
      "<API_PATH>": apiPathValue,
      "<PARENT_ID_PROP>": parentIdProp,
    });

    const ModalApi = readAndReplaceTemplate(modalApiTemplate, {
      "<NAME>": PanelName,
      "<API_PAGE_PATH>": apiUrl ? `${apiUrl}/page` : "",
      "<API_INFO_PATH>": apiUrl ? `${apiUrl}/info/` : "",
      "<API_ADD_PATH>": apiUrl ? `${apiUrl}/add` : "",
      "<API_UPD_PATH>": apiUrl ? `${apiUrl}/upd` : "",
      "<API_DEL_PATH>": apiUrl ? `${apiUrl}/del` : "",
    });

    // 创建 Modal 列表文件
    const modalFiles = [
      {
        fileName: "index.tsx",
        content: ModalPanel,
        path: panelPath,
      },
      {
        fileName: `${PanelName}Store.ts`,
        content: ModalStore,
        path: panelPath,
      },
      {
        fileName: "index.module.scss",
        content: `.${PanelName} {\n}`,
        path: panelPath,
      },
      {
        fileName: `${apiName}.ts`,
        content: ModalApi,
        path: apiPath,
      },
    ];

    modalFiles.forEach((file) => {
      writeFile(path.join(file.path, file.fileName), file.content);
    });

    // ========== 创建表单部分 ==========
    const formPanelPath = path.resolve(
      __dirname,
      "../src/pages",
      parent,
      PanelName,
      `${PanelName}Form`
    );

    const formPanelTemplate = path.resolve(__dirname, "./FormPanel/panel.js");
    const formStoreTemplate = path.resolve(__dirname, "./FormPanel/store.js");

    const FormPanel = readAndReplaceTemplate(formPanelTemplate, {
      "<NAME>": PanelName,
    });

    const FormStore = readAndReplaceTemplate(formStoreTemplate, {
      "<NAME>": PanelName,
      "<API_PATH>": apiPathValue,
    });

    // 修改 FormPanel 以支持父级ID属性
    let finalFormPanel = FormPanel;

    // 在接口中添加父级ID属性
    finalFormPanel = finalFormPanel.replace(
      /(onOk\?: \(\) => void;)/,
      `$1
  ${parentIdProp}?: number | string;`
    );

    // 在 props 解构中添加父级ID
    finalFormPanel = finalFormPanel.replace(
      /const \{ open, title, id, onCancel, onOk \} = props;/,
      `const { open, title, id, onCancel, onOk, ${parentIdProp} } = props;`
    );

    // 在 handleOk 中添加父级ID赋值（在函数开始处）
    finalFormPanel = finalFormPanel.replace(
      /(const handleOk = \(data: Record<string, unknown>\) => \{)/,
      `$1
    data.${parentIdProp} = ${parentIdProp};
`
    );

    // 创建表单文件
    const formFiles = [
      {
        fileName: "index.tsx",
        content: finalFormPanel,
        path: formPanelPath,
      },
      {
        fileName: `${PanelName}FormStore.ts`,
        content: FormStore,
        path: formPanelPath,
      },
      {
        fileName: "index.module.scss",
        content: `.${PanelName}Form {\n}`,
        path: formPanelPath,
      },
    ];

    formFiles.forEach((file) => {
      writeFile(path.join(file.path, file.fileName), file.content);
    });

    console.log(`\n✓ 成功创建 Modal 列表面板: ${PanelName}\n`);
  } catch (error) {
    console.error("\n✗ 创建 Modal 列表面板失败:", error.message);
    process.exit(1);
  }
}

main();
