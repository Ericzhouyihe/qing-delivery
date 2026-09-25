//! 滑块执行(003/T010,D4):定位选择器 + 人类轨迹生成(纯函数,可测);
//! 平台 specifics(选择器/成功判定)为实账号校准项(R11),不臆造。

/// 滑块元素定位选择器(按序尝试;实账号校准点)。
pub const SLIDER_SELECTORS: &[&str] = &[
    "#nc_1_n1z", // 淘系 nc 滑块按钮
    "#nc_1__n1z",
    "[id^='nc_'][id$='n1z']",
    ".baxia-dialog .btn_slide",
    ".slider-btn",
];

/// 单个轨迹点(视口坐标)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrajectoryPoint {
    pub x: f64,
    pub y: f64,
    pub t_ms: u32,
}

/// 生成从 (x0,y0) 到 (x1,y1) 的人类拖动轨迹:
/// 贝塞尔缓动 + 少量垂直抖动 + 随机暂停点;总时长与距离成正比。
/// 纯函数:给定相同种子输入(用简单 LCG),输出确定(可测)。
pub fn generate_trajectory(
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    seed: u64,
    steps: usize,
) -> Vec<TrajectoryPoint> {
    let steps = steps.clamp(24, 120);
    let mut rng = seed | 1; // LCG 种子(非零)
    let mut next = move || {
        rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((rng >> 33) as f64) / (u64::MAX >> 33) as f64
    };
    let dist = (x1 - x0).hypot(y1 - y0);
    let total_ms = (dist * 3.5 + 240.0) as u32; // 距离越远越慢
    let mut pts = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let p = i as f64 / steps as f64;
        // ease-out-quint:先快后慢(人类拖动特征)
        let eased = 1.0 - (1.0 - p).powi(5);
        // 控制点偏移的二次贝塞尔(轻微弧线)
        let cx = x0 + (x1 - x0) * 0.5 + (next() - 0.5) * 24.0;
        let cy = y0 + (y1 - y0) * 0.5 - 18.0;
        let x = (1.0 - eased).powi(2) * x0 + 2.0 * (1.0 - eased) * eased * cx + eased.powi(2) * x1;
        let y = (1.0 - eased).powi(2) * y0
            + 2.0 * (1.0 - eased) * eased * cy
            + eased.powi(2) * y1
            + (next() - 0.5) * 2.0 * (1.0 - p); // 越接近终点抖动越小
        pts.push(TrajectoryPoint {
            x,
            y,
            t_ms: (total_ms as f64 * p) as u32,
        });
    }
    // 1~2 个中途停顿(行为检测特征)
    if steps >= 40 {
        let pause_at = steps / 2;
        let extra = 60 + (next() * 90.0) as u32;
        for (i, pt) in pts.iter_mut().enumerate().skip(pause_at) {
            pt.t_ms += extra;
            let _ = i;
        }
    }
    pts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 轨迹端点准确_单调递进_含停顿() {
        let pts = generate_trajectory(100.0, 300.0, 400.0, 305.0, 42, 60);
        assert_eq!(pts.first().unwrap().x, 100.0);
        let last = pts.last().unwrap();
        assert!((last.x - 400.0).abs() < 6.0, "终点接近目标,实得 {}", last.x);
        // 时间单调不减
        for w in pts.windows(2) {
            assert!(w[1].t_ms >= w[0].t_ms, "时间应单调");
        }
        // y 抖动有界(±10)
        for pt in &pts {
            assert!((pt.y - 302.5).abs() < 24.0, "y 偏离过大:{}", pt.y);
        }
    }

    #[test]
    fn 同种子轨迹确定_不同种子轨迹不同() {
        let a1 = generate_trajectory(0.0, 0.0, 300.0, 0.0, 7, 50);
        let a2 = generate_trajectory(0.0, 0.0, 300.0, 0.0, 7, 50);
        let b = generate_trajectory(0.0, 0.0, 300.0, 0.0, 8, 50);
        assert_eq!(a1, a2, "同种子确定");
        assert_ne!(a1, b, "不同种子应不同");
    }
}
