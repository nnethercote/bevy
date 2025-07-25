//! Contains APIs for ordering systems and executing them on a [`World`](crate::world::World)

mod auto_insert_apply_deferred;
mod condition;
mod config;
mod executor;
mod pass;
mod schedule;
mod set;
mod stepping;

use self::graph::*;
pub use self::{condition::*, config::*, executor::*, schedule::*, set::*};
pub use pass::ScheduleBuildPass;

pub use self::graph::NodeId;

/// An implementation of a graph data structure.
pub mod graph;

/// Included optional schedule build passes.
pub mod passes {
    pub use crate::schedule::auto_insert_apply_deferred::*;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "trace")]
    use alloc::string::ToString;
    use alloc::{vec, vec::Vec};
    use core::sync::atomic::{AtomicU32, Ordering};

    pub use crate::{
        prelude::World,
        resource::Resource,
        schedule::{Schedule, SystemSet},
        system::{Res, ResMut},
    };

    #[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
    enum TestSystems {
        A,
        B,
        C,
        D,
        X,
    }

    #[derive(Resource, Default)]
    struct SystemOrder(Vec<u32>);

    #[derive(Resource, Default)]
    struct RunConditionBool(bool);

    #[derive(Resource, Default)]
    struct Counter(AtomicU32);

    fn make_exclusive_system(tag: u32) -> impl FnMut(&mut World) {
        move |world| world.resource_mut::<SystemOrder>().0.push(tag)
    }

    fn make_function_system(tag: u32) -> impl FnMut(ResMut<SystemOrder>) {
        move |mut resource: ResMut<SystemOrder>| resource.0.push(tag)
    }

    fn named_system(mut resource: ResMut<SystemOrder>) {
        resource.0.push(u32::MAX);
    }

    fn named_exclusive_system(world: &mut World) {
        world.resource_mut::<SystemOrder>().0.push(u32::MAX);
    }

    fn counting_system(counter: Res<Counter>) {
        counter.0.fetch_add(1, Ordering::Relaxed);
    }

    mod system_execution {
        use super::*;

        #[test]
        fn run_system() {
            let mut world = World::default();
            let mut schedule = Schedule::default();

            world.init_resource::<SystemOrder>();

            schedule.add_systems(make_function_system(0));
            schedule.run(&mut world);

            assert_eq!(world.resource::<SystemOrder>().0, vec![0]);
        }

        #[test]
        fn run_exclusive_system() {
            let mut world = World::default();
            let mut schedule = Schedule::default();

            world.init_resource::<SystemOrder>();

            schedule.add_systems(make_exclusive_system(0));
            schedule.run(&mut world);

            assert_eq!(world.resource::<SystemOrder>().0, vec![0]);
        }

        #[test]
        #[cfg(not(miri))]
        fn parallel_execution() {
            use alloc::sync::Arc;
            use bevy_tasks::{ComputeTaskPool, TaskPool};
            use std::sync::Barrier;

            let mut world = World::default();
            let mut schedule = Schedule::default();
            let thread_count = ComputeTaskPool::get_or_init(TaskPool::default).thread_num();

            let barrier = Arc::new(Barrier::new(thread_count));

            for _ in 0..thread_count {
                let inner = barrier.clone();
                schedule.add_systems(move || {
                    inner.wait();
                });
            }

            schedule.run(&mut world);
        }
    }

    mod system_ordering {
        use super::*;

        #[test]
        fn order_systems() {
            let mut world = World::default();
            let mut schedule = Schedule::default();

            world.init_resource::<SystemOrder>();

            schedule.add_systems((
                named_system,
                make_function_system(1).before(named_system),
                make_function_system(0)
                    .after(named_system)
                    .in_set(TestSystems::A),
            ));
            schedule.run(&mut world);

            assert_eq!(world.resource::<SystemOrder>().0, vec![1, u32::MAX, 0]);

            world.insert_resource(SystemOrder::default());

            assert_eq!(world.resource::<SystemOrder>().0, vec![]);

            // modify the schedule after it's been initialized and test ordering with sets
            schedule.configure_sets(TestSystems::A.after(named_system));
            schedule.add_systems((
                make_function_system(3)
                    .before(TestSystems::A)
                    .after(named_system),
                make_function_system(4).after(TestSystems::A),
            ));
            schedule.run(&mut world);

            assert_eq!(
                world.resource::<SystemOrder>().0,
                vec![1, u32::MAX, 3, 0, 4]
            );
        }

        #[test]
        fn order_exclusive_systems() {
            let mut world = World::default();
            let mut schedule = Schedule::default();

            world.init_resource::<SystemOrder>();

            schedule.add_systems((
                named_exclusive_system,
                make_exclusive_system(1).before(named_exclusive_system),
                make_exclusive_system(0).after(named_exclusive_system),
            ));
            schedule.run(&mut world);

            assert_eq!(world.resource::<SystemOrder>().0, vec![1, u32::MAX, 0]);
        }

        #[test]
        fn add_systems_correct_order() {
            let mut world = World::new();
            let mut schedule = Schedule::default();

            world.init_resource::<SystemOrder>();

            schedule.add_systems(
                (
                    make_function_system(0),
                    make_function_system(1),
                    make_exclusive_system(2),
                    make_function_system(3),
                )
                    .chain(),
            );

            schedule.run(&mut world);
            assert_eq!(world.resource::<SystemOrder>().0, vec![0, 1, 2, 3]);
        }

        #[test]
        fn add_systems_correct_order_nested() {
            let mut world = World::new();
            let mut schedule = Schedule::default();

            world.init_resource::<SystemOrder>();

            schedule.add_systems(
                (
                    (make_function_system(0), make_function_system(1)).chain(),
                    make_function_system(2),
                    (make_function_system(3), make_function_system(4)).chain(),
                    (
                        make_function_system(5),
                        (make_function_system(6), make_function_system(7)),
                    ),
                    (
                        (make_function_system(8), make_function_system(9)).chain(),
                        make_function_system(10),
                    ),
                )
                    .chain(),
            );

            schedule.run(&mut world);
            let order = &world.resource::<SystemOrder>().0;
            assert_eq!(
                &order[0..5],
                &[0, 1, 2, 3, 4],
                "first five items should be exactly ordered"
            );
            let unordered = &order[5..8];
            assert!(
                unordered.contains(&5) && unordered.contains(&6) && unordered.contains(&7),
                "unordered must be 5, 6, and 7 in any order"
            );
            let partially_ordered = &order[8..11];
            assert!(
                partially_ordered == [8, 9, 10] || partially_ordered == [10, 8, 9],
                "partially_ordered must be [8, 9, 10] or [10, 8, 9]"
            );
            assert_eq!(order.len(), 11, "must have exactly 11 order entries");
        }
    }

    mod conditions {

        use crate::{
            change_detection::DetectChanges,
            error::{ignore, DefaultErrorHandler, Result},
        };

        use super::*;

        #[test]
        fn system_with_condition_bool() {
            let mut world = World::default();
            let mut schedule = Schedule::default();

            world.init_resource::<RunConditionBool>();
            world.init_resource::<SystemOrder>();

            schedule.add_systems(
                make_function_system(0).run_if(|condition: Res<RunConditionBool>| condition.0),
            );

            schedule.run(&mut world);
            assert_eq!(world.resource::<SystemOrder>().0, vec![]);

            world.resource_mut::<RunConditionBool>().0 = true;
            schedule.run(&mut world);
            assert_eq!(world.resource::<SystemOrder>().0, vec![0]);
        }

        #[test]
        fn system_with_condition_result_bool() {
            let mut world = World::default();
            world.insert_resource(DefaultErrorHandler(ignore));
            let mut schedule = Schedule::default();

            world.init_resource::<SystemOrder>();

            schedule.add_systems((
                make_function_system(0).run_if(|| -> Result<bool> { Err(core::fmt::Error.into()) }),
                make_function_system(1).run_if(|| -> Result<bool> { Ok(false) }),
            ));

            schedule.run(&mut world);
            assert_eq!(world.resource::<SystemOrder>().0, vec![]);

            schedule.add_systems(make_function_system(2).run_if(|| -> Result<bool> { Ok(true) }));

            schedule.run(&mut world);
            assert_eq!(world.resource::<SystemOrder>().0, vec![2]);
        }

        #[test]
        fn systems_with_distributive_condition() {
            let mut world = World::default();
            let mut schedule = Schedule::default();

            world.insert_resource(RunConditionBool(true));
            world.init_resource::<SystemOrder>();

            fn change_condition(mut condition: ResMut<RunConditionBool>) {
                condition.0 = false;
            }

            schedule.add_systems(
                (
                    make_function_system(0),
                    change_condition,
                    make_function_system(1),
                )
                    .chain()
                    .distributive_run_if(|condition: Res<RunConditionBool>| condition.0),
            );

            schedule.run(&mut world);
            assert_eq!(world.resource::<SystemOrder>().0, vec![0]);
        }

        #[test]
        fn run_exclusive_system_with_condition() {
            let mut world = World::default();
            let mut schedule = Schedule::default();

            world.init_resource::<RunConditionBool>();
            world.init_resource::<SystemOrder>();

            schedule.add_systems(
                make_exclusive_system(0).run_if(|condition: Res<RunConditionBool>| condition.0),
            );

            schedule.run(&mut world);
            assert_eq!(world.resource::<SystemOrder>().0, vec![]);

            world.resource_mut::<RunConditionBool>().0 = true;
            schedule.run(&mut world);
            assert_eq!(world.resource::<SystemOrder>().0, vec![0]);
        }

        #[test]
        fn multiple_conditions_on_system() {
            let mut world = World::default();
            let mut schedule = Schedule::default();

            world.init_resource::<Counter>();

            schedule.add_systems((
                counting_system.run_if(|| false).run_if(|| false),
                counting_system.run_if(|| true).run_if(|| false),
                counting_system.run_if(|| false).run_if(|| true),
                counting_system.run_if(|| true).run_if(|| true),
            ));

            schedule.run(&mut world);
            assert_eq!(world.resource::<Counter>().0.load(Ordering::Relaxed), 1);
        }

        #[test]
        fn multiple_conditions_on_system_sets() {
            let mut world = World::default();
            let mut schedule = Schedule::default();

            world.init_resource::<Counter>();

            schedule.configure_sets(TestSystems::A.run_if(|| false).run_if(|| false));
            schedule.add_systems(counting_system.in_set(TestSystems::A));
            schedule.configure_sets(TestSystems::B.run_if(|| true).run_if(|| false));
            schedule.add_systems(counting_system.in_set(TestSystems::B));
            schedule.configure_sets(TestSystems::C.run_if(|| false).run_if(|| true));
            schedule.add_systems(counting_system.in_set(TestSystems::C));
            schedule.configure_sets(TestSystems::D.run_if(|| true).run_if(|| true));
            schedule.add_systems(counting_system.in_set(TestSystems::D));

            schedule.run(&mut world);
            assert_eq!(world.resource::<Counter>().0.load(Ordering::Relaxed), 1);
        }

        #[test]
        fn systems_nested_in_system_sets() {
            let mut world = World::default();
            let mut schedule = Schedule::default();

            world.init_resource::<Counter>();

            schedule.configure_sets(TestSystems::A.run_if(|| false));
            schedule.add_systems(counting_system.in_set(TestSystems::A).run_if(|| false));
            schedule.configure_sets(TestSystems::B.run_if(|| true));
            schedule.add_systems(counting_system.in_set(TestSystems::B).run_if(|| false));
            schedule.configure_sets(TestSystems::C.run_if(|| false));
            schedule.add_systems(counting_system.in_set(TestSystems::C).run_if(|| true));
            schedule.configure_sets(TestSystems::D.run_if(|| true));
            schedule.add_systems(counting_system.in_set(TestSystems::D).run_if(|| true));

            schedule.run(&mut world);
            assert_eq!(world.resource::<Counter>().0.load(Ordering::Relaxed), 1);
        }

        #[test]
        fn system_conditions_and_change_detection() {
            #[derive(Resource, Default)]
            struct Bool2(pub bool);

            let mut world = World::default();
            world.init_resource::<Counter>();
            world.init_resource::<RunConditionBool>();
            world.init_resource::<Bool2>();
            let mut schedule = Schedule::default();

            schedule.add_systems(
                counting_system
                    .run_if(|res1: Res<RunConditionBool>| res1.is_changed())
                    .run_if(|res2: Res<Bool2>| res2.is_changed()),
            );

            // both resource were just added.
            schedule.run(&mut world);
            assert_eq!(world.resource::<Counter>().0.load(Ordering::Relaxed), 1);

            // nothing has changed
            schedule.run(&mut world);
            assert_eq!(world.resource::<Counter>().0.load(Ordering::Relaxed), 1);

            // RunConditionBool has changed, but counting_system did not run
            world.get_resource_mut::<RunConditionBool>().unwrap().0 = false;
            schedule.run(&mut world);
            assert_eq!(world.resource::<Counter>().0.load(Ordering::Relaxed), 1);

            // internal state for the bool2 run criteria was updated in the
            // previous run, so system still does not run
            world.get_resource_mut::<Bool2>().unwrap().0 = false;
            schedule.run(&mut world);
            assert_eq!(world.resource::<Counter>().0.load(Ordering::Relaxed), 1);

            // internal state for bool2 was updated, so system still does not run
            world.get_resource_mut::<RunConditionBool>().unwrap().0 = false;
            schedule.run(&mut world);
            assert_eq!(world.resource::<Counter>().0.load(Ordering::Relaxed), 1);

            // now check that it works correctly changing Bool2 first and then RunConditionBool
            world.get_resource_mut::<Bool2>().unwrap().0 = false;
            world.get_resource_mut::<RunConditionBool>().unwrap().0 = false;
            schedule.run(&mut world);
            assert_eq!(world.resource::<Counter>().0.load(Ordering::Relaxed), 2);
        }

        #[test]
        fn system_set_conditions_and_change_detection() {
            #[derive(Resource, Default)]
            struct Bool2(pub bool);

            let mut world = World::default();
            world.init_resource::<Counter>();
            world.init_resource::<RunConditionBool>();
            world.init_resource::<Bool2>();
            let mut schedule = Schedule::default();

            schedule.configure_sets(
                TestSystems::A
                    .run_if(|res1: Res<RunConditionBool>| res1.is_changed())
                    .run_if(|res2: Res<Bool2>| res2.is_changed()),
            );

            schedule.add_systems(counting_system.in_set(TestSystems::A));

            // both resource were just added.
            schedule.run(&mut world);
            assert_eq!(world.resource::<Counter>().0.load(Ordering::Relaxed), 1);

            // nothing has changed
            schedule.run(&mut world);
            assert_eq!(world.resource::<Counter>().0.load(Ordering::Relaxed), 1);

            // RunConditionBool has changed, but counting_system did not run
            world.get_resource_mut::<RunConditionBool>().unwrap().0 = false;
            schedule.run(&mut world);
            assert_eq!(world.resource::<Counter>().0.load(Ordering::Relaxed), 1);

            // internal state for the bool2 run criteria was updated in the
            // previous run, so system still does not run
            world.get_resource_mut::<Bool2>().unwrap().0 = false;
            schedule.run(&mut world);
            assert_eq!(world.resource::<Counter>().0.load(Ordering::Relaxed), 1);

            // internal state for bool2 was updated, so system still does not run
            world.get_resource_mut::<RunConditionBool>().unwrap().0 = false;
            schedule.run(&mut world);
            assert_eq!(world.resource::<Counter>().0.load(Ordering::Relaxed), 1);

            // the system only runs when both are changed on the same run
            world.get_resource_mut::<Bool2>().unwrap().0 = false;
            world.get_resource_mut::<RunConditionBool>().unwrap().0 = false;
            schedule.run(&mut world);
            assert_eq!(world.resource::<Counter>().0.load(Ordering::Relaxed), 2);
        }

        #[test]
        fn mixed_conditions_and_change_detection() {
            #[derive(Resource, Default)]
            struct Bool2(pub bool);

            let mut world = World::default();
            world.init_resource::<Counter>();
            world.init_resource::<RunConditionBool>();
            world.init_resource::<Bool2>();
            let mut schedule = Schedule::default();

            schedule.configure_sets(
                TestSystems::A.run_if(|res1: Res<RunConditionBool>| res1.is_changed()),
            );

            schedule.add_systems(
                counting_system
                    .run_if(|res2: Res<Bool2>| res2.is_changed())
                    .in_set(TestSystems::A),
            );

            // both resource were just added.
            schedule.run(&mut world);
            assert_eq!(world.resource::<Counter>().0.load(Ordering::Relaxed), 1);

            // nothing has changed
            schedule.run(&mut world);
            assert_eq!(world.resource::<Counter>().0.load(Ordering::Relaxed), 1);

            // RunConditionBool has changed, but counting_system did not run
            world.get_resource_mut::<RunConditionBool>().unwrap().0 = false;
            schedule.run(&mut world);
            assert_eq!(world.resource::<Counter>().0.load(Ordering::Relaxed), 1);

            // we now only change bool2 and the system also should not run
            world.get_resource_mut::<Bool2>().unwrap().0 = false;
            schedule.run(&mut world);
            assert_eq!(world.resource::<Counter>().0.load(Ordering::Relaxed), 1);

            // internal state for the bool2 run criteria was updated in the
            // previous run, so system still does not run
            world.get_resource_mut::<RunConditionBool>().unwrap().0 = false;
            schedule.run(&mut world);
            assert_eq!(world.resource::<Counter>().0.load(Ordering::Relaxed), 1);

            // the system only runs when both are changed on the same run
            world.get_resource_mut::<Bool2>().unwrap().0 = false;
            world.get_resource_mut::<RunConditionBool>().unwrap().0 = false;
            schedule.run(&mut world);
            assert_eq!(world.resource::<Counter>().0.load(Ordering::Relaxed), 2);
        }
    }

    mod schedule_build_errors {
        use super::*;

        #[test]
        fn dependency_loop() {
            let mut schedule = Schedule::default();
            schedule.configure_sets(TestSystems::X.after(TestSystems::X));
            let mut world = World::new();
            let result = schedule.initialize(&mut world);
            assert!(matches!(result, Err(ScheduleBuildError::DependencyLoop(_))));
        }

        #[test]
        fn dependency_loop_from_chain() {
            let mut schedule = Schedule::default();
            schedule.configure_sets((TestSystems::X, TestSystems::X).chain());
            let mut world = World::new();
            let result = schedule.initialize(&mut world);
            assert!(matches!(result, Err(ScheduleBuildError::DependencyLoop(_))));
        }

        #[test]
        fn dependency_cycle() {
            let mut world = World::new();
            let mut schedule = Schedule::default();

            schedule.configure_sets(TestSystems::A.after(TestSystems::B));
            schedule.configure_sets(TestSystems::B.after(TestSystems::A));

            let result = schedule.initialize(&mut world);
            assert!(matches!(
                result,
                Err(ScheduleBuildError::DependencyCycle(_))
            ));

            fn foo() {}
            fn bar() {}

            let mut world = World::new();
            let mut schedule = Schedule::default();

            schedule.add_systems((foo.after(bar), bar.after(foo)));
            let result = schedule.initialize(&mut world);
            assert!(matches!(
                result,
                Err(ScheduleBuildError::DependencyCycle(_))
            ));
        }

        #[test]
        fn hierarchy_loop() {
            let mut schedule = Schedule::default();
            schedule.configure_sets(TestSystems::X.in_set(TestSystems::X));
            let mut world = World::new();
            let result = schedule.initialize(&mut world);
            assert!(matches!(result, Err(ScheduleBuildError::HierarchyLoop(_))));
        }

        #[test]
        fn hierarchy_cycle() {
            let mut world = World::new();
            let mut schedule = Schedule::default();

            schedule.configure_sets(TestSystems::A.in_set(TestSystems::B));
            schedule.configure_sets(TestSystems::B.in_set(TestSystems::A));

            let result = schedule.initialize(&mut world);
            assert!(matches!(result, Err(ScheduleBuildError::HierarchyCycle(_))));
        }

        #[test]
        fn system_type_set_ambiguity() {
            // Define some systems.
            fn foo() {}
            fn bar() {}

            let mut world = World::new();
            let mut schedule = Schedule::default();

            // Schedule `bar` to run after `foo`.
            schedule.add_systems((foo, bar.after(foo)));

            // There's only one `foo`, so it's fine.
            let result = schedule.initialize(&mut world);
            assert!(result.is_ok());

            // Schedule another `foo`.
            schedule.add_systems(foo);

            // When there are multiple instances of `foo`, dependencies on
            // `foo` are no longer allowed. Too much ambiguity.
            let result = schedule.initialize(&mut world);
            assert!(matches!(
                result,
                Err(ScheduleBuildError::SystemTypeSetAmbiguity(_))
            ));

            // same goes for `ambiguous_with`
            let mut schedule = Schedule::default();
            schedule.add_systems(foo);
            schedule.add_systems(bar.ambiguous_with(foo));
            let result = schedule.initialize(&mut world);
            assert!(result.is_ok());
            schedule.add_systems(foo);
            let result = schedule.initialize(&mut world);
            assert!(matches!(
                result,
                Err(ScheduleBuildError::SystemTypeSetAmbiguity(_))
            ));
        }

        #[test]
        #[should_panic]
        fn configure_system_type_set() {
            fn foo() {}
            let mut schedule = Schedule::default();
            schedule.configure_sets(foo.into_system_set());
        }

        #[test]
        fn hierarchy_redundancy() {
            let mut world = World::new();
            let mut schedule = Schedule::default();

            schedule.set_build_settings(ScheduleBuildSettings {
                hierarchy_detection: LogLevel::Error,
                ..Default::default()
            });

            // Add `A`.
            schedule.configure_sets(TestSystems::A);

            // Add `B` as child of `A`.
            schedule.configure_sets(TestSystems::B.in_set(TestSystems::A));

            // Add `X` as child of both `A` and `B`.
            schedule.configure_sets(TestSystems::X.in_set(TestSystems::A).in_set(TestSystems::B));

            // `X` cannot be the `A`'s child and grandchild at the same time.
            let result = schedule.initialize(&mut world);
            assert!(matches!(
                result,
                Err(ScheduleBuildError::HierarchyRedundancy(_))
            ));
        }

        #[test]
        fn cross_dependency() {
            let mut world = World::new();
            let mut schedule = Schedule::default();

            // Add `B` and give it both kinds of relationships with `A`.
            schedule.configure_sets(TestSystems::B.in_set(TestSystems::A));
            schedule.configure_sets(TestSystems::B.after(TestSystems::A));
            let result = schedule.initialize(&mut world);
            assert!(matches!(
                result,
                Err(ScheduleBuildError::CrossDependency(_, _))
            ));
        }

        #[test]
        fn sets_have_order_but_intersect() {
            let mut world = World::new();
            let mut schedule = Schedule::default();

            fn foo() {}

            // Add `foo` to both `A` and `C`.
            schedule.add_systems(foo.in_set(TestSystems::A).in_set(TestSystems::C));

            // Order `A -> B -> C`.
            schedule.configure_sets((
                TestSystems::A,
                TestSystems::B.after(TestSystems::A),
                TestSystems::C.after(TestSystems::B),
            ));

            let result = schedule.initialize(&mut world);
            // `foo` can't be in both `A` and `C` because they can't run at the same time.
            assert!(matches!(
                result,
                Err(ScheduleBuildError::SetsHaveOrderButIntersect(_, _))
            ));
        }

        #[test]
        fn ambiguity() {
            #[derive(Resource)]
            struct X;

            fn res_ref(_x: Res<X>) {}
            fn res_mut(_x: ResMut<X>) {}

            let mut world = World::new();
            let mut schedule = Schedule::default();

            schedule.set_build_settings(ScheduleBuildSettings {
                ambiguity_detection: LogLevel::Error,
                ..Default::default()
            });

            schedule.add_systems((res_ref, res_mut));
            let result = schedule.initialize(&mut world);
            assert!(matches!(result, Err(ScheduleBuildError::Ambiguity(_))));
        }
    }

    mod system_ambiguity {
        #[cfg(feature = "trace")]
        use alloc::collections::BTreeSet;

        use super::*;
        use crate::prelude::*;

        #[derive(Resource)]
        struct R;

        #[derive(Component)]
        struct A;

        #[derive(Component)]
        struct B;

        #[derive(BufferedEvent)]
        struct E;

        #[derive(Resource, Component)]
        struct RC;

        fn empty_system() {}
        fn res_system(_res: Res<R>) {}
        fn resmut_system(_res: ResMut<R>) {}
        fn nonsend_system(_ns: NonSend<R>) {}
        fn nonsendmut_system(_ns: NonSendMut<R>) {}
        fn read_component_system(_query: Query<&A>) {}
        fn write_component_system(_query: Query<&mut A>) {}
        fn with_filtered_component_system(_query: Query<&mut A, With<B>>) {}
        fn without_filtered_component_system(_query: Query<&mut A, Without<B>>) {}
        fn entity_ref_system(_query: Query<EntityRef>) {}
        fn entity_mut_system(_query: Query<EntityMut>) {}
        fn event_reader_system(_reader: EventReader<E>) {}
        fn event_writer_system(_writer: EventWriter<E>) {}
        fn event_resource_system(_events: ResMut<Events<E>>) {}
        fn read_world_system(_world: &World) {}
        fn write_world_system(_world: &mut World) {}

        // Tests for conflict detection

        #[test]
        fn one_of_everything() {
            let mut world = World::new();
            world.insert_resource(R);
            world.spawn(A);
            world.init_resource::<Events<E>>();

            let mut schedule = Schedule::default();
            schedule
                // nonsendmut system deliberately conflicts with resmut system
                .add_systems((resmut_system, write_component_system, event_writer_system));

            let _ = schedule.initialize(&mut world);

            assert_eq!(schedule.graph().conflicting_systems().len(), 0);
        }

        #[test]
        fn read_only() {
            let mut world = World::new();
            world.insert_resource(R);
            world.spawn(A);
            world.init_resource::<Events<E>>();

            let mut schedule = Schedule::default();
            schedule.add_systems((
                empty_system,
                empty_system,
                res_system,
                res_system,
                nonsend_system,
                nonsend_system,
                read_component_system,
                read_component_system,
                entity_ref_system,
                entity_ref_system,
                event_reader_system,
                event_reader_system,
                read_world_system,
                read_world_system,
            ));

            let _ = schedule.initialize(&mut world);

            assert_eq!(schedule.graph().conflicting_systems().len(), 0);
        }

        #[test]
        fn read_world() {
            let mut world = World::new();
            world.insert_resource(R);
            world.spawn(A);
            world.init_resource::<Events<E>>();

            let mut schedule = Schedule::default();
            schedule.add_systems((
                resmut_system,
                write_component_system,
                event_writer_system,
                read_world_system,
            ));

            let _ = schedule.initialize(&mut world);

            assert_eq!(schedule.graph().conflicting_systems().len(), 3);
        }

        #[test]
        fn resources() {
            let mut world = World::new();
            world.insert_resource(R);

            let mut schedule = Schedule::default();
            schedule.add_systems((resmut_system, res_system));

            let _ = schedule.initialize(&mut world);

            assert_eq!(schedule.graph().conflicting_systems().len(), 1);
        }

        #[test]
        fn nonsend() {
            let mut world = World::new();
            world.insert_resource(R);

            let mut schedule = Schedule::default();
            schedule.add_systems((nonsendmut_system, nonsend_system));

            let _ = schedule.initialize(&mut world);

            assert_eq!(schedule.graph().conflicting_systems().len(), 1);
        }

        #[test]
        fn components() {
            let mut world = World::new();
            world.spawn(A);

            let mut schedule = Schedule::default();
            schedule.add_systems((read_component_system, write_component_system));

            let _ = schedule.initialize(&mut world);

            assert_eq!(schedule.graph().conflicting_systems().len(), 1);
        }

        #[test]
        fn filtered_components() {
            let mut world = World::new();
            world.spawn(A);

            let mut schedule = Schedule::default();
            schedule.add_systems((
                with_filtered_component_system,
                without_filtered_component_system,
            ));

            let _ = schedule.initialize(&mut world);

            assert_eq!(schedule.graph().conflicting_systems().len(), 0);
        }

        #[test]
        fn events() {
            let mut world = World::new();
            world.init_resource::<Events<E>>();

            let mut schedule = Schedule::default();
            schedule.add_systems((
                // All of these systems clash
                event_reader_system,
                event_writer_system,
                event_resource_system,
            ));

            let _ = schedule.initialize(&mut world);

            assert_eq!(schedule.graph().conflicting_systems().len(), 3);
        }

        /// Test that when a struct is both a Resource and a Component, they do not
        /// conflict with each other.
        #[test]
        fn shared_resource_mut_component() {
            let mut world = World::new();
            world.insert_resource(RC);

            let mut schedule = Schedule::default();
            schedule.add_systems((|_: ResMut<RC>| {}, |_: Query<&mut RC>| {}));

            let _ = schedule.initialize(&mut world);

            assert_eq!(schedule.graph().conflicting_systems().len(), 0);
        }

        #[test]
        fn resource_mut_and_entity_ref() {
            let mut world = World::new();
            world.insert_resource(R);

            let mut schedule = Schedule::default();
            schedule.add_systems((resmut_system, entity_ref_system));

            let _ = schedule.initialize(&mut world);

            assert_eq!(schedule.graph().conflicting_systems().len(), 0);
        }

        #[test]
        fn resource_and_entity_mut() {
            let mut world = World::new();
            world.insert_resource(R);

            let mut schedule = Schedule::default();
            schedule.add_systems((res_system, nonsend_system, entity_mut_system));

            let _ = schedule.initialize(&mut world);

            assert_eq!(schedule.graph().conflicting_systems().len(), 0);
        }

        #[test]
        fn write_component_and_entity_ref() {
            let mut world = World::new();
            world.insert_resource(R);

            let mut schedule = Schedule::default();
            schedule.add_systems((write_component_system, entity_ref_system));

            let _ = schedule.initialize(&mut world);

            assert_eq!(schedule.graph().conflicting_systems().len(), 1);
        }

        #[test]
        fn read_component_and_entity_mut() {
            let mut world = World::new();
            world.insert_resource(R);

            let mut schedule = Schedule::default();
            schedule.add_systems((read_component_system, entity_mut_system));

            let _ = schedule.initialize(&mut world);

            assert_eq!(schedule.graph().conflicting_systems().len(), 1);
        }

        #[test]
        fn exclusive() {
            let mut world = World::new();
            world.insert_resource(R);
            world.spawn(A);
            world.init_resource::<Events<E>>();

            let mut schedule = Schedule::default();
            schedule.add_systems((
                // All 3 of these conflict with each other
                write_world_system,
                write_world_system,
                res_system,
            ));

            let _ = schedule.initialize(&mut world);

            assert_eq!(schedule.graph().conflicting_systems().len(), 3);
        }

        // Tests for silencing and resolving ambiguities
        #[test]
        fn before_and_after() {
            let mut world = World::new();
            world.init_resource::<Events<E>>();

            let mut schedule = Schedule::default();
            schedule.add_systems((
                event_reader_system.before(event_writer_system),
                event_writer_system,
                event_resource_system.after(event_writer_system),
            ));

            let _ = schedule.initialize(&mut world);

            assert_eq!(schedule.graph().conflicting_systems().len(), 0);
        }

        #[test]
        fn ignore_all_ambiguities() {
            let mut world = World::new();
            world.insert_resource(R);

            let mut schedule = Schedule::default();
            schedule.add_systems((
                resmut_system.ambiguous_with_all(),
                res_system,
                nonsend_system,
            ));

            let _ = schedule.initialize(&mut world);

            assert_eq!(schedule.graph().conflicting_systems().len(), 0);
        }

        #[test]
        fn ambiguous_with_label() {
            let mut world = World::new();
            world.insert_resource(R);

            #[derive(SystemSet, Hash, PartialEq, Eq, Debug, Clone)]
            struct IgnoreMe;

            let mut schedule = Schedule::default();
            schedule.add_systems((
                resmut_system.ambiguous_with(IgnoreMe),
                res_system.in_set(IgnoreMe),
                nonsend_system.in_set(IgnoreMe),
            ));

            let _ = schedule.initialize(&mut world);

            assert_eq!(schedule.graph().conflicting_systems().len(), 0);
        }

        #[test]
        fn ambiguous_with_system() {
            let mut world = World::new();

            let mut schedule = Schedule::default();
            schedule.add_systems((
                write_component_system.ambiguous_with(read_component_system),
                read_component_system,
            ));
            let _ = schedule.initialize(&mut world);

            assert_eq!(schedule.graph().conflicting_systems().len(), 0);
        }

        #[derive(ScheduleLabel, Hash, PartialEq, Eq, Debug, Clone)]
        struct TestSchedule;

        // Tests that the correct ambiguities were reported in the correct order.
        #[test]
        #[cfg(feature = "trace")]
        fn correct_ambiguities() {
            fn system_a(_res: ResMut<R>) {}
            fn system_b(_res: ResMut<R>) {}
            fn system_c(_res: ResMut<R>) {}
            fn system_d(_res: ResMut<R>) {}
            fn system_e(_res: ResMut<R>) {}

            let mut world = World::new();
            world.insert_resource(R);

            let mut schedule = Schedule::new(TestSchedule);
            schedule.add_systems((
                system_a,
                system_b,
                system_c.ambiguous_with_all(),
                system_d.ambiguous_with(system_b),
                system_e.after(system_a),
            ));

            schedule.graph_mut().initialize(&mut world);
            let _ = schedule.graph_mut().build_schedule(
                &mut world,
                TestSchedule.intern(),
                &BTreeSet::new(),
            );

            let ambiguities: Vec<_> = schedule
                .graph()
                .conflicts_to_string(schedule.graph().conflicting_systems(), world.components())
                .map(|item| {
                    (
                        item.0,
                        item.1,
                        item.2
                            .into_iter()
                            .map(|name| name.to_string())
                            .collect::<Vec<_>>(),
                    )
                })
                .collect();

            let expected = &[
                (
                    "system_d".to_string(),
                    "system_a".to_string(),
                    vec!["bevy_ecs::schedule::tests::system_ambiguity::R".into()],
                ),
                (
                    "system_d".to_string(),
                    "system_e".to_string(),
                    vec!["bevy_ecs::schedule::tests::system_ambiguity::R".into()],
                ),
                (
                    "system_b".to_string(),
                    "system_a".to_string(),
                    vec!["bevy_ecs::schedule::tests::system_ambiguity::R".into()],
                ),
                (
                    "system_b".to_string(),
                    "system_e".to_string(),
                    vec!["bevy_ecs::schedule::tests::system_ambiguity::R".into()],
                ),
            ];

            // ordering isn't stable so do this
            for entry in expected {
                assert!(ambiguities.contains(entry));
            }
        }

        // Test that anonymous set names work properly
        // Related issue https://github.com/bevyengine/bevy/issues/9641
        #[test]
        #[cfg(feature = "trace")]
        fn anonymous_set_name() {
            let mut schedule = Schedule::new(TestSchedule);
            schedule.add_systems((resmut_system, resmut_system).run_if(|| true));

            let mut world = World::new();
            schedule.graph_mut().initialize(&mut world);
            let _ = schedule.graph_mut().build_schedule(
                &mut world,
                TestSchedule.intern(),
                &BTreeSet::new(),
            );

            let ambiguities: Vec<_> = schedule
                .graph()
                .conflicts_to_string(schedule.graph().conflicting_systems(), world.components())
                .map(|item| {
                    (
                        item.0,
                        item.1,
                        item.2
                            .into_iter()
                            .map(|name| name.to_string())
                            .collect::<Vec<_>>(),
                    )
                })
                .collect();

            assert_eq!(
                ambiguities[0],
                (
                    "resmut_system (in set (resmut_system, resmut_system))".to_string(),
                    "resmut_system (in set (resmut_system, resmut_system))".to_string(),
                    vec!["bevy_ecs::schedule::tests::system_ambiguity::R".into()],
                )
            );
        }

        #[test]
        fn ignore_component_resource_ambiguities() {
            let mut world = World::new();
            world.insert_resource(R);
            world.allow_ambiguous_resource::<R>();
            let mut schedule = Schedule::new(TestSchedule);

            // check resource
            schedule.add_systems((resmut_system, res_system));
            schedule.initialize(&mut world).unwrap();
            assert!(schedule.graph().conflicting_systems().is_empty());

            // check components
            world.allow_ambiguous_component::<A>();
            schedule.add_systems((write_component_system, read_component_system));
            schedule.initialize(&mut world).unwrap();
            assert!(schedule.graph().conflicting_systems().is_empty());
        }
    }

    #[cfg(feature = "bevy_debug_stepping")]
    mod stepping {
        use super::*;
        use bevy_ecs::system::SystemState;

        #[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
        pub struct TestSchedule;

        macro_rules! assert_executor_supports_stepping {
            ($executor:expr) => {
                // create a test schedule
                let mut schedule = Schedule::new(TestSchedule);
                schedule
                    .set_executor_kind($executor)
                    .add_systems(|| -> () { panic!("Executor ignored Stepping") });

                // Add our schedule to stepping & and enable stepping; this should
                // prevent any systems in the schedule from running
                let mut stepping = Stepping::default();
                stepping.add_schedule(TestSchedule).enable();

                // create a world, and add the stepping resource
                let mut world = World::default();
                world.insert_resource(stepping);

                // start a new frame by running ihe begin_frame() system
                let mut system_state: SystemState<Option<ResMut<Stepping>>> =
                    SystemState::new(&mut world);
                let res = system_state.get_mut(&mut world);
                Stepping::begin_frame(res);

                // now run the schedule; this will panic if the executor doesn't
                // handle stepping
                schedule.run(&mut world);
            };
        }

        /// verify the [`SimpleExecutor`] supports stepping
        #[test]
        #[expect(deprecated, reason = "We still need to test this.")]
        fn simple_executor() {
            assert_executor_supports_stepping!(ExecutorKind::Simple);
        }

        /// verify the [`SingleThreadedExecutor`] supports stepping
        #[test]
        fn single_threaded_executor() {
            assert_executor_supports_stepping!(ExecutorKind::SingleThreaded);
        }

        /// verify the [`MultiThreadedExecutor`] supports stepping
        #[test]
        fn multi_threaded_executor() {
            assert_executor_supports_stepping!(ExecutorKind::MultiThreaded);
        }
    }
}
