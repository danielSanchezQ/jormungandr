#[macro_use]
extern crate jormungandr_scenario_tests;
extern crate jormungandr_integration_tests;

use cucumber::{after, before, cucumber, steps};
use jormungandr_scenario_tests::{
    prepare_command,
    scenario::{ContextChaCha, ProgressBarMode},
    Controller, NodeController, Seed,
};
use std::path::PathBuf;
use std::str::FromStr;

pub struct ScenarioContext {
    // You can use this struct for mutable context in scenarios.
    pub context: ContextChaCha,
    pub controller: Option<Controller>,
    pub nodes: Vec<NodeController>,
}

impl cucumber::World for ScenarioContext {}
impl std::default::Default for ScenarioContext {
    fn default() -> ScenarioContext {
        // This function is called every time a new scenario is started
        let jormungandr =
            prepare_command(PathBuf::from_str("../../target/debug/jormungandr.exe").unwrap());
        let jcli = prepare_command(PathBuf::from_str("../../target/debug/jcli.exe").unwrap());
        let context = ContextChaCha::new(
            // TODO: properly handle the seed
            Seed::from_str("10").unwrap(),
            jormungandr,
            jcli,
            None,
            false,
            ProgressBarMode::None,
        );
        ScenarioContext {
            context,
            controller: None,
            nodes: Vec::new(),
        }
    }
}

mod example_steps {
    use chain_impl_mockchain;
    use cucumber::steps;
    use jormungandr_scenario_tests::node::{LeadershipMode, PersistenceMode};
    // Any type that implements cucumber::World + Default can be the world
    steps!(crate::ScenarioContext => {
        given "The default configuration" |scenario_context, step| {
        let mut context = scenario_context.context.clone();
           let scenario_settings = prepare_scenario! {
                "Testing the network",
                &mut context,
                topology [
                    "Leader1",
                    "Passive1" -> "Leader1",
                    "Passive2" -> "Leader1",
                    "Passive3" -> "Leader1",
                    "Unknown1",
                ]
                blockchain {
                    consensus = GenesisPraos,
                    number_of_slots_per_epoch = 60,
                    slot_duration = 1,
                    leaders = [ "Leader2" ],
                    initials = [
                        account "unassigned1" with   500_000_000,
                        account "unassigned2" with   100_000_000,
                        account "delegated1" with  2_000_000_000 delegates to "Leader1",
                        account "delegated2" with    300_000_000 delegates to "Unknown1",
                    ],
                }
            };
            let mut controller =  scenario_settings.build(context).unwrap();

            scenario_context.nodes.push(controller.spawn_node("Leader1", LeadershipMode::Leader, PersistenceMode::InMemory).unwrap());
            for i in 1..=3 {
                let controller = controller.spawn_node(
                    format!("Passive{}", i).as_str(),
                    LeadershipMode::Passive,
                    PersistenceMode::InMemory,
                ).unwrap();
                scenario_context.nodes.push(controller);
            }
            controller.monitor_nodes();
            for n in &scenario_context.nodes {
                n.wait_for_bootstrap().unwrap();
            }
            scenario_context.controller = Some(controller);
        };

        when regex r"Sending from (.+) to (.+) (\d+) tokens types$" (String, String, u64) |world, from, to, amount, step| {
            // Take actions
            println!{"From {}, To {}, Amount : {}", from, to, amount};
        };

        then "Print result" |world, step| {
            // Check that the outcomes to be observed have occurred
            println!("Foo works");
        };

        // then regex r"^we can (.*) rules with regex$" |world, matches, step| {
        //     // And access them as an array
        //     assert_eq!(matches[1], "implement");
        // };
        //
        // then regex r"^we can also match (\d+) (.+) types$" (usize, String) |world, num, word, step| {
        //     // `num` will be of type usize, `word` of type String
        //     assert_eq!(num, 42);
        //     assert_eq!(word, "olika");
        // };
        //
        // then "we can use data tables to provide more parameters" |world, step| {
        //     let table = step.table().unwrap().clone();
        //
        //     assert_eq!(table.header, vec!["key", "value"]);
        //
        //     let expected_keys = table.rows.iter().map(|row| row[0].to_owned()).collect::<Vec<_>>();
        //     let expected_values = table.rows.iter().map(|row| row[1].to_owned()).collect::<Vec<_>>();
        //
        //     assert_eq!(expected_keys, vec!["a", "b"]);
        //     assert_eq!(expected_values, vec!["fizz", "buzz"]);
        // };
    });
}

// Declares a before handler function named `a_before_fn`
before!(a_before_fn => |scenario| {

});

// Declares an after handler function named `an_after_fn`
after!(an_after_fn => |scenario| {
    // let scenario_context = &scenario.world;
    // for n in scenario_context.nodes {
    //     n.shutdown().unwrap();
    // }
    // scenario_context.as_ref.unwrap().finalize();
});

// A setup function to be called before everything else
fn setup() {}

cucumber! {
    features: "./features", // Path to our feature files
    world: crate::ScenarioContext, // The world needs to be the same for steps and the main cucumber call
    steps: &[
        example_steps::steps // the `steps!` macro creates a `steps` function in a module
    ],
    setup: setup, // Optional; called once before everything
    before: &[
        a_before_fn // Optional; called before each scenario
    ],
    after: &[
        an_after_fn // Optional; called after each scenario
    ]
}
