use crate::DistortionModel;

pub fn distort_normalized(model: DistortionModel, coeffs: &[f64], x: f64, y: f64) -> (f64, f64) {
    match model {
        DistortionModel::None => (x, y),
        DistortionModel::BrownConrady => distort_brown_conrady(coeffs, x, y),
        DistortionModel::Fisheye => distort_fisheye(coeffs, x, y),
    }
}

pub fn undistort_normalized(model: DistortionModel, coeffs: &[f64], x: f64, y: f64) -> (f64, f64) {
    if matches!(model, DistortionModel::None) || coeffs.is_empty() {
        return (x, y);
    }
    let mut xu = x;
    let mut yu = y;
    for _ in 0..8 {
        let (xd, yd) = distort_normalized(model, coeffs, xu, yu);
        xu += x - xd;
        yu += y - yd;
    }
    (xu, yu)
}

fn distort_brown_conrady(coeffs: &[f64], x: f64, y: f64) -> (f64, f64) {
    let k1 = coeffs.first().copied().unwrap_or(0.0);
    let k2 = coeffs.get(1).copied().unwrap_or(0.0);
    let p1 = coeffs.get(2).copied().unwrap_or(0.0);
    let p2 = coeffs.get(3).copied().unwrap_or(0.0);
    let k3 = coeffs.get(4).copied().unwrap_or(0.0);
    let k4 = coeffs.get(5).copied().unwrap_or(0.0);
    let k5 = coeffs.get(6).copied().unwrap_or(0.0);
    let k6 = coeffs.get(7).copied().unwrap_or(0.0);

    let r2 = x * x + y * y;
    let r4 = r2 * r2;
    let r6 = r4 * r2;

    let radial_num = 1.0 + k1 * r2 + k2 * r4 + k3 * r6;
    let radial_den = 1.0 + k4 * r2 + k5 * r4 + k6 * r6;
    let radial = if radial_den.abs() > 1e-12 {
        radial_num / radial_den
    } else {
        radial_num
    };

    let x_tan = 2.0 * p1 * x * y + p2 * (r2 + 2.0 * x * x);
    let y_tan = p1 * (r2 + 2.0 * y * y) + 2.0 * p2 * x * y;

    (x * radial + x_tan, y * radial + y_tan)
}

fn distort_fisheye(coeffs: &[f64], x: f64, y: f64) -> (f64, f64) {
    let k1 = coeffs.first().copied().unwrap_or(0.0);
    let k2 = coeffs.get(1).copied().unwrap_or(0.0);
    let k3 = coeffs.get(2).copied().unwrap_or(0.0);
    let k4 = coeffs.get(3).copied().unwrap_or(0.0);

    let r = (x * x + y * y).sqrt();
    if r < 1e-12 {
        return (x, y);
    }

    let theta = r.atan();
    let theta2 = theta * theta;
    let theta4 = theta2 * theta2;
    let theta6 = theta4 * theta2;
    let theta8 = theta4 * theta4;

    let theta_d = theta * (1.0 + k1 * theta2 + k2 * theta4 + k3 * theta6 + k4 * theta8);
    let scale = theta_d / r;
    (x * scale, y * scale)
}
