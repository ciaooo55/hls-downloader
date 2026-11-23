//! Fit task columns to the available window width without persisted geometry.

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub id: &'static str,
    pub label: &'static str,
    pub width: f32,
}

pub fn columns(viewport: f32) -> Vec<Column> {
    let viewport = if viewport.is_finite() { viewport } else { 0.0 };
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
            width: if compact { 112.0 } else { 170.0 },
        },
        Column {
            id: "status",
            label: "状态",
            width: if compact { 84.0 } else { 116.0 },
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
            width: if compact { 112.0 } else { 150.0 },
        });
    }
    columns.push(Column {
        id: "actions",
        label: "操作",
        width: if compact { 96.0 } else { 104.0 },
    });
    columns[0].width = (viewport - columns.iter().map(|column| column.width).sum::<f32>()).max(0.0);
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
    fn unmeasured_viewport_keeps_column_widths_finite_and_nonnegative() {
        for viewport in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(columns(viewport)
                .iter()
                .all(|column| column.width.is_finite() && column.width >= 0.0));
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
