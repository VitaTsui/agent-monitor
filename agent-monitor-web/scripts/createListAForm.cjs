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
 * 创建列表面板（带表单）
 * 用法: node createListAForm.cjs <panelName> [parent] [apiUrl]
 * 示例: node createListAForm.cjs userManagement dataMonitoring /vm/user
 */
function main() {
  try {
    const panelName = process.argv[2];
    validatePanelName(panelName);

    const PanelName = uppercamelcase(panelName);
    const parent = process.argv[3] || "";
    const apiUrl = process.argv[4] || "";

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

    console.log(`\n开始创建列表面板（带表单）: ${PanelName}`);
    console.log(`面板路径: ${panelPath}`);
    console.log(`API 路径: ${apiPathValue}`);
    console.log(`权限前缀: ${permiPrefix || "无"}\n`);

    // ========== 创建列表部分 ==========
    const listPanelTemplate = path.resolve(
      __dirname,
      "./ListPanel/listAform.js"
    );
    const listStoreTemplate = path.resolve(__dirname, "./ListPanel/store.js");
    const listApiTemplate = path.resolve(__dirname, "./ListPanel/api.js");

    const ListPanel = readAndReplaceTemplate(listPanelTemplate, {
      "<NAME>": PanelName,
      "<ADD_PERMI>": permiPrefix ? `"${permiPrefix}:add"` : "",
      "<EDIT_PERMI>": permiPrefix ? `"${permiPrefix}:upd"` : "",
      "<DELETE_PERMI>": permiPrefix ? `"${permiPrefix}:del"` : "",
      "<QUERY_PERMI>": permiPrefix ? `"${permiPrefix}:query"` : "",
    });

    const ListStore = readAndReplaceTemplate(listStoreTemplate, {
      "<NAME>": PanelName,
      "<API_PATH>": apiPathValue,
    });

    const ListApi = readAndReplaceTemplate(listApiTemplate, {
      "<NAME>": PanelName,
      "<API_PAGE_PATH>": apiUrl ? `${apiUrl}/page` : "",
      "<API_INFO_PATH>": apiUrl ? `${apiUrl}/info/` : "",
      "<API_ADD_PATH>": apiUrl ? `${apiUrl}/add` : "",
      "<API_UPD_PATH>": apiUrl ? `${apiUrl}/upd` : "",
      "<API_DEL_PATH>": apiUrl ? `${apiUrl}/del` : "",
    });

    // 创建列表文件
    const listFiles = [
      {
        fileName: "index.tsx",
        content: ListPanel,
        path: panelPath,
      },
      {
        fileName: `${PanelName}Store.ts`,
        content: ListStore,
        path: panelPath,
      },
      {
        fileName: "index.module.scss",
        content: `.${PanelName} {\n}`,
        path: panelPath,
      },
      {
        fileName: `${apiName}.ts`,
        content: ListApi,
        path: apiPath,
      },
    ];

    listFiles.forEach((file) => {
      writeFile(path.join(file.path, file.fileName), file.content);
    });

    // ========== 创建表单部分 ==========
    const formPanelPath = path.resolve(
      __dirname,
      "../src/pages",
      parent,
      PanelName.toLowerCase(),
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

    // 创建表单文件
    const formFiles = [
      {
        fileName: "index.tsx",
        content: FormPanel,
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

    console.log(`\n✓ 成功创建列表面板（带表单）: ${PanelName}\n`);
  } catch (error) {
    console.error("\n✗ 创建列表面板（带表单）失败:", error.message);
    process.exit(1);
  }
}

main();
