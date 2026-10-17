//! `/factory/item-settings` (Bilimbi `Web.ItemSettingsLive`): the company's own
//! item statuses and default currency, beside the effective values.
use super::*;
use crate::ItemSettings as Settings;
use bilibi_ui::{Fact, Facts, Field, InputKind};

#[derive(Clone, Default)]
struct Form {
    statuses: String,
    currency: String,
}

pub(super) struct ItemSettings {
    notice: Notice,
    form: Form,
    settings: Option<Settings>,
}

impl ItemSettings {
    pub(super) async fn open(ctx: &Ctx<'_>) -> Result<Self> {
        let mut page = Self {
            notice: Notice::default(),
            form: Form::default(),
            settings: None,
        };
        page.load(ctx).await?;
        Ok(page)
    }

    /// Reads the effective settings and refills the form with the company's
    /// own overrides.
    pub(super) async fn load(&mut self, ctx: &Ctx<'_>) -> Result<()> {
        self.settings = None;
        self.form = Form::default();
        if !ctx.can_view().await? {
            self.notice
                .say("You do not have permission to view item settings.");
            return Ok(());
        }
        let inventory = ctx.inventory();
        let loaded = async {
            Ok::<_, crate::Error>((
                inventory.item_settings(ctx.scope, ctx.company).await?,
                inventory
                    .item_setting_overrides(ctx.scope, ctx.company)
                    .await?,
            ))
        }
        .await;
        if let Some((settings, overrides)) = self.notice.take(loaded)? {
            self.settings = Some(settings);
            self.form = Form {
                statuses: overrides.statuses.unwrap_or_default().join("\n"),
                currency: overrides.default_currency_code.unwrap_or_default(),
            };
            self.notice.error = None;
        }
        Ok(())
    }

    pub(super) async fn event(&mut self, ctx: &Ctx<'_>, value: &Value) -> Result<()> {
        match event(value)? {
            "clear-flash" => self.notice.flash = None,
            "save_settings" => {
                if !ctx.can_manage().await? {
                    self.notice.denied("manage item settings");
                    return Ok(());
                }
                self.save(ctx, value).await?;
            }
            _ => return Err(Error::Invalid),
        }
        Ok(())
    }

    async fn save(&mut self, ctx: &Ctx<'_>, value: &Value) -> Result<()> {
        let posted = params::form_values(value, "settings");
        let form = Form {
            statuses: posted.get("statuses").cloned().unwrap_or_default(),
            currency: posted.get("currency").cloned().unwrap_or_default(),
        };
        let statuses: Vec<String> = form
            .statuses
            .split('\n')
            .map(|status| status.trim().to_owned())
            .filter(|status| !status.is_empty())
            .collect();
        let currency = form.currency.trim().to_uppercase();
        let new = Settings {
            statuses: (!statuses.is_empty()).then_some(statuses),
            default_currency_code: (!currency.is_empty()).then_some(currency),
        };
        let saved = ctx
            .inventory()
            .configure_item_settings(ctx.scope, ctx.company, &new)
            .await;
        if self.notice.take(saved)?.is_some() {
            self.load(ctx).await?;
            self.notice.saved("Item settings saved.");
        } else {
            self.form = form;
        }
        Ok(())
    }

    pub(super) async fn render(&self, ctx: &Ctx<'_>) -> Result<String> {
        let manage = ctx.can_manage().await?;
        let mut card = if manage {
            render_form(&self.form)?
        } else {
            Html::EMPTY
        };
        if let Some(settings) = &self.settings {
            card.push(&render(Facts {
                id: "item-settings-facts",
                items: vec![
                    Fact {
                        title: "Effective item statuses",
                        value: text(&settings.statuses.clone().unwrap_or_default().join(", ")),
                        ..Fact::default()
                    },
                    Fact {
                        title: "Effective default currency",
                        value: text(
                            settings
                                .default_currency_code
                                .as_deref()
                                .unwrap_or("Not set"),
                        ),
                        ..Fact::default()
                    },
                ],
            })?);
        }
        let body = render(Card {
            id: "item-settings-card",
            title: "Company overrides",
            class: "mt-5",
            body: card,
            ..Card::default()
        })?;
        layout(
            "item-settings-index",
            PageVariant::Form,
            Header {
                title: text("Item settings"),
                subtitle: "Set the company's item statuses and default currency.",
                ..Header::default()
            },
            "item-settings-error",
            &self.notice,
            body,
        )
    }
}

fn render_form(form: &Form) -> Result<Html> {
    #[derive(Template)]
    #[template(
        source = r#"<form id="item-settings-form" data-event="save_settings" class="space-y-4 p-4">{{ statuses|safe }}{{ currency|safe }}{{ save|safe }}</form>"#,
        ext = "html"
    )]
    struct Fields {
        statuses: Html,
        currency: Html,
        save: Html,
    }
    render(Fields {
        statuses: render(
            Field::in_form("settings", "statuses")
                .kind(InputKind::Textarea)
                .label("Item statuses")
                .hint("One status per line. The first is the default. Leave empty to inherit the tenant setting or allow any status.")
                .value(&form.statuses),
        )?,
        currency: render(
            Field::in_form("settings", "currency")
                .label("Default currency")
                .hint("Three-letter currency code. Leave empty to inherit or require a currency on each item.")
                .value(&form.currency),
        )?,
        save: render(Button {
            variant: ButtonVariant::Primary,
            button_type: "submit",
            body: text("Save settings"),
            ..Button::default()
        })?,
    })
}
