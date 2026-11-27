//! Compact attitude and flight instruments for pilot and exterior views.
use crate::aircraft::Instruments;
use egui::{Align2, Color32, Pos2, Stroke, Vec2};

fn attitude(ui: &mut egui::Ui, flight: Instruments) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(62.), egui::Sense::hover());
    let painter = ui.painter();
    let center = rect.center();
    let radius = 28.;
    let sky = Color32::from_rgb(59, 105, 131);
    let earth = Color32::from_rgb(130, 91, 60);
    let ink = Color32::from_gray(232);
    let marker = Stroke::new(2.5, Color32::from_rgb(255, 196, 86));
    let (sin, cos) = flight.bank_degrees.to_radians().sin_cos();
    let on_ball = |x: f32, y: f32| -> Pos2 {
        center + radius * egui::vec2(cos * x + sin * y, -sin * x + cos * y)
    };
    let pitch = flight.pitch_degrees / 45.;
    painter.circle_filled(center, radius, sky);
    if pitch <= -1. {
        painter.circle_filled(center, radius, earth);
    } else if pitch < 1. {
        let start = pitch.asin();
        let arc = std::f32::consts::PI - 2. * start;
        let mut ground = Vec::with_capacity(33);
        for i in 0..=32 {
            let a = start + arc * i as f32 / 32.;
            ground.push(on_ball(a.cos(), a.sin()));
        }
        painter.add(egui::Shape::convex_polygon(ground, earth, Stroke::NONE));
        let x = (1. - pitch * pitch).sqrt();
        painter.line_segment(
            [on_ball(-x, pitch), on_ball(x, pitch)],
            Stroke::new(1.3, ink),
        );
    }
    for marking in [-20., -10., 10., 20.] {
        let y = pitch - marking / 45.;
        if y.abs() < 0.88 {
            let width = if marking == 10. || marking == -10. {
                0.20
            } else {
                0.31
            };
            painter.line_segment(
                [on_ball(-width, y), on_ball(width, y)],
                Stroke::new(1., ink),
            );
        }
    }
    painter.circle_stroke(center, radius, Stroke::new(1.3, Color32::from_gray(158)));
    for sign in [-1., 1.] {
        let wing = center + egui::vec2(sign * 8., 0.);
        painter.line_segment([center + egui::vec2(sign * 17., 0.), wing], marker);
        painter.line_segment([wing, wing + egui::vec2(0., 3.)], marker);
    }
    painter.circle_filled(center, 1.7, marker.color);
}

pub(super) fn show(ctx: &egui::Context, flight: Instruments) {
    egui::Area::new(egui::Id::new("aircraft instruments"))
        .anchor(Align2::LEFT_BOTTOM, [14., -14.])
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    attitude(ui, flight);
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{:03.0}°   {:.0} kt   ·   {:.0}% power",
                                flight.heading_degrees.round().rem_euclid(360.),
                                flight.speed_mps * 1.94384,
                                flight.throttle * 100.
                            ))
                            .strong(),
                        );
                        ui.label(format!(
                            "{:.0} ft   {:+.0} ft/min",
                            flight.altitude_m * 3.28084,
                            flight.climb_mps * 196.85
                        ));
                        if let (Some(power), Some(gear)) = (flight.twin, flight.gear_status) {
                            ui.small(format!(
                                "{:.0} kt TAS   ·   L {:.0}% / R {:.0}%   ·   {}",
                                flight.true_speed_mps * 1.94384,
                                power[0] * 100.,
                                power[1] * 100.,
                                gear
                            ));
                        }
                        if flight.show_airflow {
                            ui.small("Y · Airflow");
                            ui.horizontal(|ui| {
                                ui.colored_label(Color32::from_rgb(126, 227, 255), "Tracer");
                                ui.colored_label(Color32::from_rgb(100, 250, 150), "Lift");
                                ui.colored_label(Color32::from_rgb(255, 197, 107), "Drag");
                                ui.colored_label(Color32::from_rgb(255, 110, 90), "Separation");
                            });
                        }
                        if flight.flaps_degrees > 0.5 || flight.pitch_trim.abs() > 0.005 {
                            ui.small(format!(
                                "Flaps {:.0}°   Trim {:+.0}%",
                                flight.flaps_degrees,
                                400. * flight.pitch_trim
                            ));
                        }
                        if flight.grounded {
                            ui.small("Q/E steer · Space brake · V exit");
                        } else if flight.stall_warning {
                            ui.colored_label(
                                Color32::from_rgb(255, 196, 86),
                                "STALL · high angle of attack",
                            );
                        } else {
                            ui.small(format!(
                                "Bank {:+.0}°   Pitch {:+.0}°",
                                flight.bank_degrees, flight.pitch_degrees
                            ));
                        }
                    });
                });
            });
        });
}
