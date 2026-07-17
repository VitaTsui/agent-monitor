`
import React, { useEffect, useState } from "react";

import { ChakraButtonProps, ColumnsType, FormItemProps, Panel } from "@hsu-react/ui";
import <NAME>Store from "./<NAME>Store";
import { observer } from "mobx-react-lite";
import styles from "./index.module.scss";
import <NAME>Form from "./<NAME>Form";
import { PlusOutlined } from "@ant-design/icons";
import { Operate } from "@hsu-react/ui";

const <NAME>: React.FC = observer(() => {
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
  } = <NAME>Store;
  const [id, setId] = useState<number | string>();
  const [open, setOpen] = useState(false);
  const [title, setTitle] = useState("");

  useEffect(() => {
    initSearchData();
  }, [initSearchData]);

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
      <Panel.List
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
