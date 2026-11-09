//! Fit task columns to the available window width without persisted geometry.

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub id: &'static str,
    pub label: &'static str,
    pub width: f32,
}

pub fn columns(viewport: f32) -> Vec<Column> {
    let compact = viewport < 930.0;
    let mut columns = vec![
        Column {
            id: "name",
            label: "名称",
            width: 0.0,
        },
        Column {
            id: "progress",
            label: "进度",
            width: if compact { 100.0 } else { 180.0 },
        },
        Column {
            id: "status",
            label: "状态",
            width: if compact { 84.0 } else { 110.0 },
        },
    ];
    if viewport >= 850.0 {
        columns.push(Column {
            id: "speed",
            label: "速度",
            width: if compact { 84.0 } else { 100.0 },
        });
    }
    if viewport >= 700.0 {
        columns.push(Column {
            id: "size",
            label: "大小",
            width: if compact { 92.0 } else { 110.0 },
        });
    }
    columns.push(Column {
        id: "actions",
        label: "操作",
        width: if compact { 88.0 } else { 100.0 },
    });
    columns[0].width = viewport - columns.iter().map(|column| column.width).sum::<f32>();
    columns
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_fit_every_supported_width_and_keep_primary_actions() {
        // Minimum workbench width minus compact rail, padding and scrollbar space.
        for viewport in 453..=1920 {
            let columns = columns(viewport as f32);
            assert_eq!(
                columns.iter().map(|column| column.width).sum::<f32>(),
                viewport as f32
            );
            assert!(columns[0].width >= 160.0);
            for id in ["name", "progress", "status", "actions"] {
                assert!(columns.iter().any(|column| column.id == id));
            }
        }
    }

    #[test]
    fn compact_columns_return_when_the_window_expands() {
        let wide = columns(1200.0);
        let narrow = columns(453.0);
        assert!(!narrow
            .iter()
            .any(|column| matches!(column.id, "size" | "speed")));
        assert!(wide.iter().any(|column| column.id == "size"));
        assert!(wide.iter().any(|column| column.id == "speed"));
        assert_eq!(columns(1200.0), wide);
        assert!(narrow[1].width < wide[1].width);
    }
}
