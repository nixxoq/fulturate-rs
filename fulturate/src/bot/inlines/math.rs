use crate::bot::modules::math::MathSettings;
use crate::core::config::Config;
use crate::{bot::modules::Owner, core::db::schemas::settings::Settings, errors::MyError};
use eidolon_lang::interpreter::value::EidolonValue;
use eidolon_lang::interpreter::evaluate;
use image::{ImageBuffer, ImageFormat, Rgb};
use plotters::backend::BitMapBackend;
use plotters::chart::ChartBuilder;
use plotters::drawing::IntoDrawingArea;
use plotters::prelude::{IntoFont, LineSeries, RED, WHITE};
use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;
use teloxide::dispatching::dialogue::GetChatId;
use teloxide::types::{InlineQueryResultCachedPhoto, InputFile};
use teloxide::{
    payloads::AnswerInlineQuerySetters,
    prelude::*,
    types::{
        InlineQuery, InlineQueryResult, InlineQueryResultArticle,
        InputMessageContent, InputMessageContentText,
    },
};

fn generate_plot(expression: &str) -> Result<Vec<u8>, MyError> {
    let width = 800;
    let height = 600;
    let mut pixel_buffer: Vec<u8> = vec![0; (width * height * 3) as usize];

    let ast =
        eidolon_lang::parse_eidolon_source(expression).map_err(|e| MyError::Other(e.message))?;

    {
        let root = BitMapBackend::with_buffer(&mut pixel_buffer, (width, height)).into_drawing_area();
        root.fill(&WHITE).map_err(|e| MyError::Plotting(e.to_string()))?;

        let mut chart = ChartBuilder::on(&root)
            .caption(format!("y = {}", expression), <(&str, _)>::into_font(("sans-serif", 40)))
            .margin(10)
            .x_label_area_size(40)
            .y_label_area_size(40)
            .build_cartesian_2d(-10f64..10f64, -5f64..5f64)
            .map_err(|e| MyError::Plotting(e.to_string()))?;

        chart
            .configure_mesh()
            .draw()
            .map_err(|e| MyError::Plotting(e.to_string()))?;

        chart
            .draw_series(LineSeries::new(
                (-500..=500).map(|i| i as f64 / 50.0).map(|x| {
                    let mut context = HashMap::new();
                    context.insert("x".to_string(), EidolonValue::Number(x));
                    let y_val = evaluate(&ast, &context)
                        .ok()
                        .and_then(|v| v.as_number(0).ok())
                        .unwrap_or(f64::NAN);
                    (x, y_val)
                }),
                &RED,
            ))
            .map_err(|e| MyError::Plotting(e.to_string()))?;

        root.present().map_err(|e| MyError::Plotting(e.to_string()))?;
    }

    let mut png_buffer: Vec<u8> = Vec::new();
    let image_buffer: ImageBuffer<Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_raw(width, height, pixel_buffer)
            .ok_or_else(|| MyError::Plotting("Failed to create image buffer from raw pixels".to_string()))?;

    image_buffer
        .write_to(&mut Cursor::new(&mut png_buffer), ImageFormat::Png)
        .map_err(|e| MyError::Plotting(format!("Failed to encode image to PNG: {}", e)))?;

    Ok(png_buffer)
}

pub async fn handle_math_inline(bot: Bot, q: InlineQuery, config: Arc<Config>) -> Result<(), MyError> {
    let expression = q.query.trim();

    if expression.is_empty() {
        let help_article = InlineQueryResultArticle::new(
            "math_help",
            "Как использовать математику?",
            InputMessageContent::Text(InputMessageContentText::new(
                "Введите простое выражение (2+2) или функцию с переменной (sin(x)), чтобы построить график.",
            )),
        )
            .description("Например: 2*2, 5!, sin(pi/2), x^2-cos(x)");

        bot.answer_inline_query(q.id, vec![InlineQueryResult::Article(help_article)])
            .cache_time(10)
            .await?;
        return Ok(());
    }

    let result: InlineQueryResult;

    if expression.to_lowercase().contains('x') {
        match generate_plot(expression) {
            Ok(image_bytes) => {
                let archive_chat_id: i64 = config.get_archive_chat_id().parse().unwrap();
                let photo_file = InputFile::memory(image_bytes);

                println!("{}, {:?}", archive_chat_id, photo_file);

                let message = bot
                    .send_photo(ChatId(archive_chat_id), photo_file)
                    .await?;

                let file_id = message
                    .photo()
                    .and_then(|photos| photos.last().map(|p| p.file.id.clone()))
                    .unwrap();

                let photo_result = InlineQueryResultCachedPhoto::new(
                    "plot_result",
                    file_id
                )
                    .title(format!("График для y = {}", expression))
                    .description("Нажмите, чтобы отправить график в чат.");
                result = InlineQueryResult::CachedPhoto(photo_result);
                bot.delete_message(message.chat_id().unwrap(), message.id).await?;
            }
            Err(e) => {
                result = InlineQueryResult::Article(InlineQueryResultArticle::new(
                    "plot_error",
                    format!("Ошибка построения: {}", e),
                    InputMessageContent::Text(InputMessageContentText::new(
                        format!("Произошла ошибка при построении графика: {}", e)
                    ))
                ));
            }
        }
    } else {
        let article = match eidolon_lang::parse_eidolon_source(expression) {
            Ok(ast) => match evaluate(&ast, &HashMap::new()) {
                Ok(calc_result) => InlineQueryResultArticle::new(
                    "calc_result",
                    format!("Результат: {}", calc_result),
                    InputMessageContent::Text(InputMessageContentText::new(format!(
                        "{} = {}",
                        expression, calc_result
                    ))),
                ),
                Err(e) => InlineQueryResultArticle::new(
                    "calc_error",
                    format!("Ошибка вычисления: {}", e.message),
                    InputMessageContent::Text(InputMessageContentText::new(format!(
                        "Произошла ошибка при вычислении: {}",
                        e.message
                    ))),
                ),
            },
            Err(e) => InlineQueryResultArticle::new(
                "parse_error",
                format!("Ошибка: {}", e.message),
                InputMessageContent::Text(InputMessageContentText::new(format!(
                    "Произошла ошибка: {}",
                    e.message
                ))),
            ),
        };
        result = InlineQueryResult::Article(article);
    }

    bot.answer_inline_query(q.id, vec![result]).await?;

    Ok(())
}

pub async fn is_math_query(q: InlineQuery) -> bool {
    let owner = Owner {
        id: q.from.id.to_string(),
        r#type: "user".to_string(),
    };

    let settings = Settings::get_module_settings::<MathSettings>(&owner, "math")
        .await
        .unwrap();
    settings.enabled
}