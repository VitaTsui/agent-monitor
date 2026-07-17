`
import React, { useEffect, useState } from "react";

import { ChakraButtonProps, ColumnsType, FormItemProps, Panel } from "@hsu-react/ui";
import <NAME>Store from "./<NAME>Store";
import { observer } from "mobx-react-lite";
import styles from "./index.module.scss";
import <NAME>Form from "./<NAME>Form";
import { PlusOutlined } from "@ant-design/icons";
import { Operate } from "@hsu-react/ui";

interface <NAME>Props {
  open?: boolean;
  onCancel?: () => void;
  <PARENT_ID_PROP>?: number | string;
}

const <NAME>: React.FC<<NAME>Props> = observer((props) => {
  const {
    open: <NAME_LOWER>Open,
    onCancel: on<NAME>Cancel,
    <PARENT_ID_PROP>,
  } = props;
  const {
    setSearchData,
    initSearchData,
    dataSource,
    isLoading,
    total,
    changePage,
    page,
    getDataSource,
    delData,
    order,
    onOrderChange,
    resetStore,
  } = <NAME>Store;
  const [id, setId] = useState<number | string>();
  const [open, setOpen] = useState(false);
  const [title, setTitle] = useState("");

  useEffect(() => {
    if (<NAME_LOWER>Open && <PARENT_ID_PROP>) {
      initSearchData({ <PARENT_ID_PROP> });
    }
  }, [initSearchData, <NAME_LOWER>Open, <PARENT_ID_PROP>]);

  const searchItems: FormItemProps[] = [];

  const beforeButtonGroup: ChakraButtonProps[] = [
    {
      title: "新增",
      colorPalette: "blue",
      icon: <PlusOutlined />,
      onClick: () => {
        setTitle("新增");
        setOpen(true);
      },
      hasPermi: [<ADD_PERMI>],
    },
  ];

  const columns: ColumnsType = [
    {
      title: "操作",
      width: 140,
      ellipsis: false,
      align: "center",
      fixed: "right",
      fixedWidth: true,
      render: (record) => (
        <Operate
          menu={[
            {
              title: "编辑",
              onClick: () => {
                setTitle("修改");
                setOpen(true);
                setId(record.id);
              },
              hasPermi: [<EDIT_PERMI>],
            },
            {
              title: "删除",
              delete: true,
              onConfirm: () => {
                delData(record.id);
              },
              hasPermi: [<DELETE_PERMI>],
            },
          ]}
        />
      ),
    },
  ];

  return (
    <>
      <Panel.List.Modal
        open={<NAME_LOWER>Open}
        title="<MODAL_TITLE>"
        onCancel={() => {
          resetStore();
          on<NAME>Cancel?.();
        }}
        footer={false}
        className={styles.<NAME>}
        searchProps={{
          searchItems,
          onSearch: setSearchData,
          onReset: initSearchData,
          beforeButtonGroup,
          hasPermi: [<QUERY_PERMI>],
        }}
        tableProps={{
          columns,
          dataSource,
          rowKey: "id",
          loading: isLoading,
          pagination: {
            total,
            onChange: (num, size) => changePage({ num, size }),
            current: page?.num,
            pageSize: page?.size,
          },
          order,
          onOrderChange,
        }}
      />
      <<NAME>Form
        open={open}
        title={title}
        id={id}
        <PARENT_ID_PROP>={<PARENT_ID_PROP>}
        onCancel={() => {
          setId("");
          setOpen(false);
        }}
        onOk={() => {
          getDataSource();
        }}
      />
    </>
  );
});

export default <NAME>;
`;
